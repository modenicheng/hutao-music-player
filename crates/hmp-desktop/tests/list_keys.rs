//! 曲目表键盘导航集成测试（testing backend + 合成按键/指针，单测试单进程）。
//!
//! 守护 M7「焦点可达」的列表 roving 光标（ui/track-table.slint）：
//! 1. 整张表是一个 Tab 停靠点，获得焦点即有可见光标（首个可见行）；
//! 2. ↑/↓ 移动光标行并**滚动跟随**——光标走出视口时页面跟着滚（窗口化虚拟化
//!    下窗外的行并未实例化，光标按绝对下标记，不受行窗影响）；
//! 3. Enter 播放光标行；
//! 4. ←/→ 行内子光标到达歌手/专辑链接，Enter 激活；无参数的链接被跳过；
//! 5. 歌手链接命中区收在歌手名文本宽内：点标题列里歌名右侧的空白是「播这一
//!    行」，不再被当成歌手链接带走。
//!
//! 载体 = 最近播放页：页内只有返回键与曲目表，从窗口根作用域 Shift+Tab 倒着
//! 走「关闭 → 最大化 → 最小化 → 音量 → 播放 → 音质徽章」后即落在表上
//! （共 7 次；不依赖真实媒体库的歌单数量）。页面视口高由测试显式回写（648 = 800 − 顶栏带 40 −
//! 播放条带 112），让窗口化虚拟化与滚动跟随按真实几何工作。
//!
//! 几何（1280×800）：内容浮板 (240..1272, 40..688)。页内距顶 24 + 页头 84 +
//! 间距 16 → 表头行内容坐标 y=124，行距 41（39 + 2）。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, SharedString, platform::WindowEvent};

use hmp_desktop::{Data, Nav, Player, Route, TrackRow, Viewport};

fn settle(ms: u32) {
    for _ in 0..(ms / 16).max(1) {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn key(ui: &hmp_desktop::AppWindow, k: slint::platform::Key, pressed: bool) {
    let text: SharedString = k.into();
    ui.window().dispatch_event(if pressed {
        WindowEvent::KeyPressed { text }
    } else {
        WindowEvent::KeyReleased { text }
    });
}

fn tap(ui: &hmp_desktop::AppWindow, k: impl Into<SharedString>) {
    let k: SharedString = k.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: k.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: k });
}

fn shift_tab(ui: &hmp_desktop::AppWindow) {
    key(ui, slint::platform::Key::Shift, true);
    tap(ui, slint::platform::Key::Tab);
    key(ui, slint::platform::Key::Shift, false);
}

fn click(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    for pressed in [true, false] {
        ui.window().dispatch_event(if pressed {
            WindowEvent::PointerPressed {
                position,
                button: slint::platform::PointerEventButton::Left,
            }
        } else {
            WindowEvent::PointerReleased {
                position,
                button: slint::platform::PointerEventButton::Left,
            }
        });
    }
}

#[test]
fn track_table_roving_cursor_and_links() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );
    settle(320);

    let nav = Nav::get(&ui);
    let navs: Arc<Mutex<Vec<(Route, String)>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let navs = Arc::clone(&navs);
        let ui_weak = ui.as_weak();
        nav.on_navigate(move |route, param| {
            navs.lock().unwrap().push((route, param.to_string()));
            if let Some(ui) = ui_weak.upgrade() {
                let nav = Nav::get(&ui);
                nav.set_route(route);
                nav.set_param(param);
            }
        });
    }
    let plays: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let plays = Arc::clone(&plays);
        Player::get(&ui).on_play_tracks(move |_, start| plays.lock().unwrap().push(start));
    }

    // 400 行；偶数行带歌手链接参数，奇数行没有（子光标须跳过不可用的链接）
    let rows: Vec<TrackRow> = (0..400)
        .map(|i| TrackRow {
            mid: format!("row-{i}").into(),
            source: 0,
            title: format!("曲{i}").into(),
            artists: "歌手".into(),
            artist_mid: if i % 2 == 0 {
                format!("a-{i}").into()
            } else {
                "".into()
            },
            album: "专辑".into(),
            album_mid: format!("b-{i}").into(),
            duration_ms: 200_000,
            quality: "".into(),
        })
        .collect();
    let count = rows.len() as i32;
    Data::get(&ui).set_recent(slint::ModelRc::new(slint::VecModel::from(rows)));
    Data::get(&ui).set_recent_count(count);
    nav.invoke_navigate(Route::Recent, "".into());
    Viewport::get(&ui).set_view_h(648.0);
    settle(200);
    assert_eq!(nav.get_route(), Route::Recent);

    // —— 1. 落到表上：获得焦点即有光标（首个可见行 = 第 0 行）———
    for _ in 0..7 {
        shift_tab(&ui);
    }
    // —— 2/3. ↓×3 → 光标在第 3 行，Enter 播放它 ———
    for _ in 0..3 {
        tap(&ui, slint::platform::Key::DownArrow);
    }
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *plays.lock().unwrap(),
        vec![3],
        "Enter 应播放光标行（第 3 行）"
    );

    // —— 滚动跟随：↓×40 → 第 43 行在视口外，页面滚到它完整可见 ———
    for _ in 0..40 {
        tap(&ui, slint::platform::Key::DownArrow);
    }
    settle(200);
    // 行 43 底 = 124 + 43×41 + 39 = 1926；滚到视口底（内容坐标 648）→ 偏移 −1278
    let scrolled = ui.get_page_scroll_y();
    assert!(
        (scrolled + 1278.0).abs() < 4.0,
        "光标走出视口时页面应滚动跟随，实际偏移 {scrolled}"
    );
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        plays.lock().unwrap().last(),
        Some(&43),
        "深处的行按绝对下标播放"
    );

    // —— ↑×43 回第 0 行：滚回表头贴视口顶（偏移 −124）———
    for _ in 0..43 {
        tap(&ui, slint::platform::Key::UpArrow);
    }
    settle(200);
    let back = ui.get_page_scroll_y();
    assert!(
        (back + 124.0).abs() < 4.0,
        "光标回到顶部时页面应滚回表头，实际偏移 {back}"
    );
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(plays.lock().unwrap().last(), Some(&0));

    // —— 4. 行内子光标：→ 歌手链接，Enter 激活；← 回到行本身 ———
    // （激活链接会导航离开，本页随之卸载：之后空格必须仍是播放/暂停——
    //   焦点已交还根作用域，不随被卸载的列表一起丢失）
    let toggles: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    {
        let toggles = Arc::clone(&toggles);
        Player::get(&ui).on_toggle_play(move || {
            *toggles.lock().unwrap() += 1;
        });
    }
    let navs_before = navs.lock().unwrap().len();
    tap(&ui, slint::platform::Key::RightArrow);
    tap(&ui, slint::platform::Key::RightArrow);
    tap(&ui, slint::platform::Key::LeftArrow);
    tap(&ui, slint::platform::Key::Return);
    {
        let n = navs.lock().unwrap();
        assert_eq!(
            n.len(),
            navs_before + 1,
            "→ → ← 后 Enter = 歌手链接（子光标可退回）"
        );
        assert_eq!(n[navs_before], (Route::Artist, "a-0".to_string()));
    }
    tap(&ui, " ");
    assert_eq!(*toggles.lock().unwrap(), 1, "页面卸载后空格应仍是播放/暂停");

    // 回到本页、重新落到表上：无歌手链接的行 → 直接到专辑链接
    nav.invoke_navigate(Route::Recent, "".into());
    settle(200);
    for _ in 0..7 {
        shift_tab(&ui);
    }
    tap(&ui, slint::platform::Key::DownArrow);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        plays.lock().unwrap().last(),
        Some(&1),
        "↓ 后 Enter 播放该行"
    );
    tap(&ui, slint::platform::Key::RightArrow);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        navs.lock().unwrap().last(),
        Some(&(Route::Album, "b-1".to_string())),
        "无歌手链接的行 → 跳到专辑链接"
    );
    // 再回到表头贴顶的初始状态，供命中区检查
    nav.invoke_navigate(Route::Recent, "".into());
    settle(200);

    // —— 5. 歌手命中区：标题列里歌名右侧的空白 = 播放本行（不跳歌手页）———
    // 重新导航已归顶：行 5 窗口坐标 y = 40 + 24 + 84 + 16 + 41 + 5×41 = 410..449；
    // 标题列 x = 340..844，歌名与歌手名都很短，x=700 处是空白
    let navs_before = navs.lock().unwrap().len();
    let plays_before = plays.lock().unwrap().len();
    click(&ui, 700.0, 430.0);
    assert_eq!(
        plays.lock().unwrap().last(),
        Some(&5),
        "点标题列空白应播放该行"
    );
    assert_eq!(plays.lock().unwrap().len(), plays_before + 1);
    assert_eq!(
        navs.lock().unwrap().len(),
        navs_before,
        "标题列空白不得被当成歌手链接"
    );
}
