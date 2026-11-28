//! 播放页 overlay 模态焦点陷阱（testing backend + 合成按键，单测试单进程）。
//!
//! 守护 M7 收口（AUDIT §29「Tab 仍会经过被遮盖的主界面控件」）：
//! 1. overlay 打开时 Tab 从窗口根作用域出发落在 overlay 第一站（喜欢键），
//!    不进入被遮盖的侧栏/内容区/播放条；
//! 2. Shift+Tab 从 overlay 第一站倒着走，落在标题栏窗口钮（既有设计：窗口钮
//!    始终可用），再 Tab 环绕回 overlay 第一站——模态环 = overlay + 标题栏；
//! 3. 指针按下让出焦点后（KeyNav.focus-released → 根作用域）不变量仍成立：
//!    Tab 仍只进 overlay；
//! 4. 关闭 overlay 后焦点回打开它的播放条「打开播放页」控件：直接回车即再次
//!    请求打开（而不是停在根作用域、回车无人接收）。
//!
//! 焦点断言走行为面（不新增查询 API）：模态环第一站 = 播放页喜欢键 →
//! `NowPlaying.toggle-liked`；背景侧栏首站 = 「首页」→ Nav.navigate。计数器
//! 分叉即泄漏。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, SharedString, platform::WindowEvent};

use hmp_desktop::{Nav, NowPlaying, Player};

fn settle(ms: u32) {
    for _ in 0..(ms / 16).max(1) {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn tap(ui: &hmp_desktop::AppWindow, key: impl Into<SharedString>) {
    let key: SharedString = key.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key });
}

fn shift_tab(ui: &hmp_desktop::AppWindow) {
    let shift: SharedString = slint::platform::Key::Shift.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: shift.clone() });
    tap(ui, slint::platform::Key::Tab);
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: shift });
}

#[test]
fn overlay_traps_focus_and_returns_to_opener() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );
    settle(320);

    // overlay 舞台/控制台按 Player.has-track 条件实例化：播种一首在播曲
    let player = Player::get(&ui);
    player.set_has_track(true);
    player.set_title("测试曲".into());
    player.set_artists("测试歌手".into());
    player.set_album("测试专辑".into());
    player.set_duration_ms(200_000);
    player.set_can_previous(true);
    player.set_can_next(true);

    let liked = Arc::new(Mutex::new(0usize));
    {
        let liked = Arc::clone(&liked);
        NowPlaying::get(&ui).on_toggle_liked(move || *liked.lock().unwrap() += 1);
    }
    let navs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let navs = Arc::clone(&navs);
        Nav::get(&ui).on_navigate(move |route, param| {
            navs.lock()
                .unwrap()
                .push(format!("{route:?}:{param}"));
        });
    }
    let toggles = Arc::new(Mutex::new(0usize));
    {
        let toggles = Arc::clone(&toggles);
        player.on_toggle_play(move || *toggles.lock().unwrap() += 1);
    }
    let opens = Arc::new(Mutex::new(0usize));
    {
        let opens = Arc::clone(&opens);
        player.on_show_overlay(move || *opens.lock().unwrap() += 1);
    }

    // —— 1. Tab 从根作用域出发 → overlay 第一站（喜欢键），不落侧栏 ——
    player.set_overlay_visible(true);
    settle(320);
    tap(&ui, slint::platform::Key::Tab);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(*liked.lock().unwrap(), 1, "Tab 首站应为播放页喜欢键");
    assert!(
        navs.lock().unwrap().is_empty(),
        "Tab 不得进入被遮盖的主界面（侧栏首页会发导航）"
    );
    assert_eq!(*toggles.lock().unwrap(), 0, "Tab 不得落到播放条播放键");

    // —— 2. Shift+Tab 反向环绕：标题栏窗口钮 ↔ overlay 首站 ——
    // 若反向环仍含主界面，Shift+Tab 会落在播放条（下一个 Tab 不再回到喜欢键）。
    shift_tab(&ui); // 标题栏最后一个窗口钮（关闭）
    tap(&ui, slint::platform::Key::Tab); // 环绕 → overlay 首站
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *liked.lock().unwrap(),
        2,
        "反向环绕应回 overlay 首站（标题栏窗口钮仍在环内）"
    );

    // —— 3. 指针让出焦点（KeyTouch clear-focus → 根作用域）后不变量保持 ——
    hmp_desktop::keynav::pointer_activity(&ui);
    settle(64);
    shift_tab(&ui); // 根作用域倒着走 = 标题栏关闭
    tap(&ui, slint::platform::Key::Tab);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *liked.lock().unwrap(),
        3,
        "指针活动后 Tab 仍应只进模态层"
    );

    // —— 4. 关闭 overlay：焦点回打开它的播放条控件 ———
    player.set_overlay_visible(false);
    settle(320);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *opens.lock().unwrap(),
        1,
        "关闭后回车应命中「打开播放页」控件（焦点回触发器）"
    );
    assert_eq!(*toggles.lock().unwrap(), 0, "回车不得误触播放/暂停");
}
