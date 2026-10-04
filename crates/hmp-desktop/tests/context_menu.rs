//! 统一右键菜单链路回归（testing backend + 合成右键，单测试单进程）。
//!
//! 守护三件事：
//! 1. 曲目行右键**释放** → TrackMenu 快照（context/index/mid）与命中行一致；
//!    index 与左键 play-tracks 的行下标互为印证（同一命中点，几何漂移免疫：
//!    先用左键标定指针处行下标，再在同点右键断言快照）。
//! 2. 未接入宿主（侧栏行 HoverItem 未开 context-enabled）右键 → 无动作，
//!    快照保持哨兵值——"其余部分无动作"契约。
//! 3. 全局开关（TrackMenu.enabled=false）→ 已接入行右键也无动作。
//!
//! TestingBackend 无 Viewport 绑定 → TrackTable 全量渲染回退（window-start=0），
//! 行下标即模型下标。

use slint::{ComponentHandle, Global, LogicalPosition, platform::WindowEvent};

/// 内容浮板中部的曲目行（library 页：PageHeader 84 + 统计 72 + 表头 39 后，
/// 首行顶 ≈ 窗口 y=293、行距 41 → y=350 落在第 2 行中部，避开行界与表头）
const ROW_X: f32 = 640.0;
const ROW_Y: f32 = 350.0;
/// 侧栏展开区（x=8..232）内一点：导航行 HoverItem 未接入右键
const SIDEBAR_X: f32 = 100.0;
const SIDEBAR_Y: f32 = 300.0;

fn settle() {
    for _ in 0..40 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn press(ui: &hmp_desktop::AppWindow, x: f32, y: f32, button: slint::platform::PointerEventButton) {
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: LogicalPosition::new(x, y),
        button,
    });
}

fn release(ui: &hmp_desktop::AppWindow, x: f32, y: f32, button: slint::platform::PointerEventButton) {
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(x, y),
        button,
    });
}

#[test]
fn context_menu_snapshot_and_gates() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data(
        &ui,
        &std::sync::Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );

    // 播种曲库（mid 带下标便于断言快照）；默认路由 library → 我喜欢表
    {
        let data = hmp_desktop::Data::get(&ui);
        let rows: Vec<hmp_desktop::TrackRow> = (0..30)
            .map(|i| hmp_desktop::TrackRow {
                mid: format!("menu-track-{i}").into(),
                source: 0,
                title: format!("曲目 {i}").into(),
                artists: "测试歌手".into(),
                // 行内歌手/专辑链接全禁用：右键路径走行宿主（链接转发路径
                // 属视觉/真机验证面），测试命中点稳定不依赖链接区几何
                artist_mid: "".into(),
                album: "测试专辑".into(),
                album_mid: "".into(),
                duration_ms: 200_000,
                quality: "".into(),
            })
            .collect();
        let count = rows.len() as i32;
        data.set_liked(slint::ModelRc::new(slint::VecModel::from(rows)));
        data.set_liked_count(count);
    }
    settle();

    let menu = hmp_desktop::TrackMenu::get(&ui);
    let player = hmp_desktop::Player::get(&ui);

    // —— 0. 左键标定：指针处行下标（play-tracks 回带）———
    let plays: std::sync::Arc<std::sync::Mutex<Vec<i32>>> = std::sync::Arc::new(
        std::sync::Mutex::new(Vec::new()),
    );
    {
        let plays = std::sync::Arc::clone(&plays);
        player.on_play_tracks(move |_tracks, start| {
            plays.lock().unwrap().push(start);
        });
    }
    press(&ui, ROW_X, ROW_Y, slint::platform::PointerEventButton::Left);
    release(&ui, ROW_X, ROW_Y, slint::platform::PointerEventButton::Left);
    settle();
    let hit_index = *plays
        .lock()
        .unwrap()
        .last()
        .expect("左键应命中曲目行触发 play-tracks");

    // —— 1. 曲目行右键释放：快照与命中行一致 ———
    press(&ui, ROW_X, ROW_Y, slint::platform::PointerEventButton::Right);
    release(&ui, ROW_X, ROW_Y, slint::platform::PointerEventButton::Right);
    settle();
    assert!(
        matches!(menu.get_context(), hmp_desktop::MenuContext::Library),
        "通用列表右键应为 library 上下文"
    );
    assert_eq!(menu.get_index(), hit_index, "快照下标应与左键标定行一致");
    assert_eq!(
        menu.get_track().mid,
        format!("menu-track-{hit_index}"),
        "快照 mid 应为命中行"
    );

    // —— 2. 未接入宿主（侧栏行）右键：无动作 ———
    // 哨兵：把快照改写为非常规值，右键后不得被覆盖
    menu.set_index(-1);
    press(&ui, SIDEBAR_X, SIDEBAR_Y, slint::platform::PointerEventButton::Right);
    release(&ui, SIDEBAR_X, SIDEBAR_Y, slint::platform::PointerEventButton::Right);
    settle();
    assert_eq!(menu.get_index(), -1, "侧栏右键不得写入快照（context-enabled 默认关）");

    // —— 3. 全局开关关：已接入行右键也无动作 ———
    menu.set_enabled(false);
    menu.set_index(-2);
    press(&ui, ROW_X, ROW_Y, slint::platform::PointerEventButton::Right);
    release(&ui, ROW_X, ROW_Y, slint::platform::PointerEventButton::Right);
    settle();
    assert_eq!(menu.get_index(), -2, "全局关闸后曲目行右键不得写入快照");
}
