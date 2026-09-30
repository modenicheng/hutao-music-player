//! HoverGroup/HoverItem 滑动高亮链路回归测试（testing backend + 合成指针）。
//!
//! 机制背景（读 i-slint-core-1.17 input.rs/input_items.rs 确证）：
//! TouchArea 对 Moved 事件返回 EventAccepted 会中止向更低兄弟的遍历，
//! has-hover 由命中的最顶层 TouchArea 独占、只沿祖先链传播——
//! 因此 HoverItem 的 TouchArea 必须包裹行内容（祖先），本测试守护该结构：
//! - Button 满行覆盖时行级 hover 仍上报（祖先 TouchArea 感知）
//! - 总线几何随行滑动更新
//! - 离开所有行（无新行接管）后总线自校验收起
//! - 行内按钮点击不被行级 TouchArea 截获
//!
//! 注意：testing backend 的 platform 是进程级单例，本文件只放一个 #[test]。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global};

fn move_to(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(x, y),
        });
}

fn scroll(ui: &hmp_desktop::AppWindow, x: f32, y: f32, dy: f32, notches: u32) {
    for _ in 0..notches {
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(x, y),
                delta_x: 0.0,
                delta_y: dy,
            });
    }
}

fn press_release(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    for kind in [
        slint::platform::WindowEvent::PointerPressed {
            position: slint::LogicalPosition::new(x, y),
            button: slint::platform::PointerEventButton::Left,
        },
        slint::platform::WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(x, y),
            button: slint::platform::PointerEventButton::Left,
        },
    ] {
        ui.window().dispatch_event(kind);
    }
}

#[test]
fn hover_slider_chain_reports_and_clears() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data(&ui);

    // 侧栏歌单区已接真数据（Data.sidebar-*）：测试环境无库为空 → 手动播种，
    // 让歌单行 hover 链路的结构覆盖与媒体库数据来源解耦。
    {
        let data = hmp_desktop::Data::get(&ui);
        data.set_sidebar_created(slint::ModelRc::new(slint::VecModel::from(vec![
            hmp_desktop::PlaylistCover {
                id: "901".into(),
                name: "测试歌单甲".into(),
                image: hmp_desktop::covers::cover_image("playlist:901"),
            },
            hmp_desktop::PlaylistCover {
                id: "902".into(),
                name: "测试歌单乙".into(),
                image: hmp_desktop::covers::cover_image("playlist:902"),
            },
        ])));
        data.set_sidebar_favorited(slint::ModelRc::new(slint::VecModel::from(vec![
            hmp_desktop::PlaylistCover {
                id: "903".into(),
                name: "收藏歌单".into(),
                image: hmp_desktop::covers::cover_image("playlist:903"),
            },
        ])));
        // 我喜欢列表同样播种：内容页滚动命中的行此前隐式依赖真库
        // "最近播放预览"区块（预览已从我喜欢页移除），改由合成行承载。
        let rows: Vec<hmp_desktop::TrackRow> = (0..6)
            .map(|i| hmp_desktop::TrackRow {
                mid: format!("liked-{i}").into(),
                source: 0,
                title: format!("喜欢曲目 {i}").into(),
                artists: "测试歌手".into(),
                artist_mid: "".into(),
                album: "".into(),
                album_mid: "".into(),
                duration_ms: 200_000,
                quality: "".into(),
                cover: slint::Image::default(),
            })
            .collect();
        data.set_liked(slint::ModelRc::new(slint::VecModel::from(rows)));
    }

    let nav_route = Arc::new(Mutex::new(hmp_desktop::Route::Library));
    {
        let nav_route = Arc::clone(&nav_route);
        hmp_desktop::Nav::get(&ui).on_navigate(move |route, _| {
            *nav_route.lock().unwrap() = route;
        });
    }

    let bus = hmp_desktop::HoverBus::get(&ui);
    assert!(!bus.get_active(), "初始应无人上报");

    // 步进推进 mock 时间让初始布局/动画（侧栏展开等）收敛，再合成指针事件——
    // 否则命中测试落在动画中途的布局代际上，行几何与视觉不一致
    for _ in 0..40 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }

    // 悬停主导航第一行（首页，顶栏下移后窗口 y=92..132；Button 覆盖整行）：
    // 祖先 TouchArea 必须仍感知并按行几何上报
    move_to(&ui, 120.0, 112.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!(bus.get_active(), "满行 Button 之下行级 hover 应激活总线");
    assert!(
        (bus.get_item_pos().y - 92.0).abs() < 0.5,
        "第一行 y 应为 92，实际 {}",
        bus.get_item_pos().y
    );
    assert!((bus.get_item_pos().x - 8.0).abs() < 0.5);
    assert!((bus.get_item_height() - 40.0).abs() < 0.5);

    // 滑到第三行（排行榜，y=180..220）：几何随行更新
    move_to(&ui, 120.0, 200.0);
    assert!(bus.get_active());
    assert!(
        (bus.get_item_pos().y - 180.0).abs() < 0.5,
        "第三行 y 应为 180，实际 {}",
        bus.get_item_pos().y
    );

    // 歌单条目行（36px 纯内容行，自建歌单第一项）：同样上报
    // （主导航含搜索项共 9 行；歌单区在其下，顶栏下移后首行 y=580）
    move_to(&ui, 120.0, 580.0);
    assert!(bus.get_active(), "歌单条目行应上报");
    assert!(
        (bus.get_item_height() - 36.0).abs() < 0.5,
        "歌单行高应为 36，实际 {}",
        bus.get_item_height()
    );

    // 移到播放条（无 HoverItem 区域）：自校验退出应收起总线
    move_to(&ui, 640.0, 770.0);
    assert!(!bus.get_active(), "离开所有行后总线应收起");

    // 回到行上重新点亮
    move_to(&ui, 120.0, 112.0);
    assert!(bus.get_active());

    // 行内按钮点击不被行级 TouchArea 截获：点排行榜行 → navigate(top)
    press_release(&ui, 120.0, 200.0);
    assert_eq!(*nav_route.lock().unwrap(), hmp_desktop::Route::Top);

    // —— 内容页滚动后的对齐不变量 ——
    // 滚动后逐点悬停：上报矩形必须包含指针位置（若 absolute-position 绑定
    // 对 Flickable viewport 偏移失活/漏算，上报几何会偏离指针可判）
    let mut checked = 0;
    let mut last_report: Option<(f32, f32)> = None;
    scroll(&ui, 640.0, 300.0, 120.0, 12);
    for y in (120..660).step_by(16) {
        move_to(&ui, 640.0, y as f32);
        if bus.get_active() {
            let p = bus.get_item_pos();
            let (w, h) = (bus.get_item_width(), bus.get_item_height());
            assert!(
                p.x <= 640.0 && 640.0 <= p.x + w && p.y <= y as f32 && y as f32 <= p.y + h,
                "上报矩形 ({p:?} {w}x{h}) 未包含指针 (640, {y})"
            );
            checked += 1;
            last_report = Some((p.x, p.y));
        }
    }
    assert!(checked > 0, "内容页滚动后应仍能命中行（checked={checked}）");
    println!("scroll containment checked={checked} last={last_report:?}");

    // Equalizer 共享时钟随 mock 时间推进（Timer→属性→绑定链路存活）
    hmp_desktop::Player::get(&ui).set_playing(true);
    let eq = hmp_desktop::EqClock::get(&ui);
    let t0 = eq.get_t();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(100));
    let t1 = eq.get_t();
    assert!(t1 > t0, "EqClock 应随 mock 时间前进：{t0:?} → {t1:?}");
}
