//! 导航滚动位置 + 全局快捷键集成测试（testing backend，单测试单进程）。
//!
//! 守护 M7 的导航语义：
//! 1. **前进归顶**：从长列表中部点进详情页，新页落在顶部（此前路由切换从不
//!    碰滚动偏移，新页落在旧偏移处）；
//! 2. **返回恢复**：返回时回到离开时的位置（此前落在详情页遗留的偏移）；
//! 3. **Alt+Left = 页头返回键**：库页子页（最近播放等）回「我喜欢」，其余
//!    走历史栈；顶层页无操作——否则「我喜欢 → 最近播放 → 返回」会再退回
//!    最近播放（历史栈里记着它）；
//! 4. **Ctrl+F**：跳到搜索页并请求聚焦搜索框。
//!
//! 走真实的 `bridge::bind`（历史栈在那里）。出网隔离：HMP_IPC_ENDPOINT
//! 指向不存在的 socket，在线内容请求快速失败；库页数据由本测试播种，
//! 不依赖真实媒体库内容（`bind_offline` + 空快照，导航不再读 data_dir）。

use std::sync::Arc;

use slint::{ComponentHandle, Global, SharedString, platform::WindowEvent};

use hmp_desktop::{Data, KeyNav, Nav, Route, TrackRow, Viewport};

fn settle(ms: u32) {
    for _ in 0..(ms / 16).max(1) {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn key(ui: &hmp_desktop::AppWindow, text: &str, pressed: bool) {
    let text = SharedString::from(text);
    ui.window().dispatch_event(if pressed {
        WindowEvent::KeyPressed { text }
    } else {
        WindowEvent::KeyReleased { text }
    });
}

fn tap(ui: &hmp_desktop::AppWindow, text: &str) {
    key(ui, text, true);
    key(ui, text, false);
}

/// 按住 modifier 敲一下 key（winit 把修饰键作为独立按键事件送入）
fn with_modifier(ui: &hmp_desktop::AppWindow, modifier: slint::platform::Key, text: &str) {
    let modifier: SharedString = modifier.into();
    let modifier = modifier.as_str();
    key(ui, modifier, true);
    tap(ui, text);
    key(ui, modifier, false);
}

fn scroll_to(ui: &hmp_desktop::AppWindow, y: f32) {
    Viewport::get(ui).invoke_scroll_to(y);
}

#[test]
fn navigation_scroll_position_and_shortcuts() {
    // SAFETY: 单线程测试进程内串行执行（本文件仅一个测试）；值唯一。
    unsafe {
        std::env::set_var(
            "HMP_IPC_ENDPOINT",
            "/tmp/hmp-test-nav-scroll-bogus/hmp.sock",
        );
    }
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let runtime = Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap());
    hmp_desktop::bridge::load_data_offline(&ui, &runtime);
    let prefs = Arc::new(std::sync::Mutex::new(hmp_desktop::prefs::Prefs::default()));
    hmp_desktop::bridge::bind_offline(&ui, prefs, Arc::clone(&runtime));

    // 我喜欢：400 行（内容高 ≫ 视口 648），离开位置可分辨
    let rows: Vec<TrackRow> = (0..400)
        .map(|i| TrackRow {
            mid: format!("nav-{i}").into(),
            source: 0,
            title: format!("曲目 {i}").into(),
            artists: "测试歌手".into(),
            artist_mid: "".into(),
            album: "".into(),
            album_mid: "".into(),
            duration_ms: 200_000,
            quality: "".into(),
        })
        .collect();
    let count = rows.len() as i32;
    Data::get(&ui).set_liked(slint::ModelRc::new(slint::VecModel::from(rows)));
    Data::get(&ui).set_liked_count(count);
    settle(320);

    let nav = Nav::get(&ui);
    assert_eq!(nav.get_route(), Route::Library);

    // —— 1/2. 离开位置 -2000 → 进详情页归顶 → Alt+Left 返回恢复 ———
    scroll_to(&ui, -2000.0);
    settle(400);
    assert!(
        (ui.get_page_scroll_y() + 2000.0).abs() < 1.0,
        "预置滚动应生效，实际 {}",
        ui.get_page_scroll_y()
    );

    nav.invoke_navigate(Route::Album, "zz-不存在的专辑-zz".into());
    settle(400);
    assert_eq!(nav.get_route(), Route::Album);
    assert!(
        ui.get_page_scroll_y().abs() < 1.0,
        "前进到新页应归顶，实际 {}",
        ui.get_page_scroll_y()
    );

    with_modifier(
        &ui,
        slint::platform::Key::Alt,
        &<SharedString as From<_>>::from(slint::platform::Key::LeftArrow),
    );
    settle(400);
    assert_eq!(nav.get_route(), Route::Library, "Alt+Left 应返回上一页");
    assert!(
        (ui.get_page_scroll_y() + 2000.0).abs() < 1.0,
        "返回应恢复离开时的位置，实际 {}",
        ui.get_page_scroll_y()
    );

    // —— 3. Alt+Left 与页头返回键同一语义：库页子页回「我喜欢」；顶层页不回退 ———
    nav.invoke_navigate(Route::Recent, "".into());
    settle(80);
    assert_eq!(nav.get_route(), Route::Recent);
    with_modifier(
        &ui,
        slint::platform::Key::Alt,
        &<SharedString as From<_>>::from(slint::platform::Key::LeftArrow),
    );
    settle(80);
    assert_eq!(
        nav.get_route(),
        Route::Library,
        "最近播放的返回 = 回「我喜欢」"
    );
    // 历史栈里仍有「最近播放」：顶层页再按 Alt+Left 不得退回去（乒乓）
    with_modifier(
        &ui,
        slint::platform::Key::Alt,
        &<SharedString as From<_>>::from(slint::platform::Key::LeftArrow),
    );
    settle(80);
    assert_eq!(
        nav.get_route(),
        Route::Library,
        "顶层页的 Alt+Left 应无操作"
    );

    // —— 4. Ctrl+F：跳到搜索页并请求聚焦搜索框 ———
    let focus_before = KeyNav::get(&ui).get_focus_search_seq();
    with_modifier(&ui, slint::platform::Key::Control, "f");
    settle(80);
    assert_eq!(nav.get_route(), Route::Search, "Ctrl+F 应跳到搜索页");
    assert_eq!(
        KeyNav::get(&ui).get_focus_search_seq(),
        focus_before + 1,
        "Ctrl+F 应请求聚焦搜索框"
    );
    // 历史栈照常工作：搜索页的上一步是「最近播放」（Ctrl+F 是一次前进导航）
    nav.invoke_back();
    settle(80);
    assert_eq!(nav.get_route(), Route::Recent);
    nav.invoke_back();
    settle(80);
    assert_eq!(nav.get_route(), Route::Library);
}
