//! 键盘导航集成测试（testing backend + 合成按键/指针，单测试单进程）。
//!
//! 守护 M7「焦点可达」的地基机制（ui/keynav.slint 焦点模型）：
//! 1. Tab 按声明序遍历键盘停靠点，Enter / 空格激活聚焦控件；
//! 2. 聚焦控件上的空格是「按下该控件」，不冒泡成播放/暂停；
//! 3. 指针活动（`keynav::pointer_activity`，生产路径 = winit 按下钩子）后
//!    控件让出焦点，空格回到播放/暂停——否则「Tab 到某键 → 改用鼠标 →
//!    按空格想暂停」会误触残留焦点的控件；
//! 4. 鼠标点击不聚焦也不被吞：键盘聚焦某控件后，首次点击另一控件即生效
//!    （FocusScope 点击聚焦会 accept Pressed，KeyTouch 因此做成 TouchArea
//!    的祖先并关闭 focus-on-click）。
//!
//! 几何（1280×800、title-bar-height=32、layout-gap=8、侧栏展开宽 224、
//! 账户面板 52）：导航行 40px + 间距 4，自 y=92 起——
//! 首页 92..132 / 发现 136..176 / 排行榜 180..220 / 搜索 224..264。
//!
//! 导航回调由本测试自接（只记录 + 回写路由），不挂 bridge::bind——那会
//! 向真实 daemon 端点发在线内容请求。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, SharedString, platform::WindowEvent};

use hmp_desktop::{Data, Nav, Player, Route};

fn settle() {
    for _ in 0..40 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

/// 按下并释放一个键
fn tap(ui: &hmp_desktop::AppWindow, text: &str) {
    press(ui, text);
    release(ui, text);
}

fn press(ui: &hmp_desktop::AppWindow, text: &str) {
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: SharedString::from(text),
    });
}

fn release(ui: &hmp_desktop::AppWindow, text: &str) {
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: SharedString::from(text),
    });
}

fn special(key: slint::platform::Key) -> String {
    <SharedString as From<_>>::from(key).to_string()
}

fn tab(ui: &hmp_desktop::AppWindow) {
    tap(ui, &special(slint::platform::Key::Tab));
}

fn shift_tab(ui: &hmp_desktop::AppWindow) {
    let shift = special(slint::platform::Key::Shift);
    press(ui, &shift);
    tap(ui, &special(slint::platform::Key::Tab));
    release(ui, &shift);
}

fn click(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    for kind in [
        WindowEvent::PointerPressed {
            position: slint::LogicalPosition::new(x, y),
            button: slint::platform::PointerEventButton::Left,
        },
        WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(x, y),
            button: slint::platform::PointerEventButton::Left,
        },
    ] {
        ui.window().dispatch_event(kind);
    }
}

#[test]
fn keyboard_focus_activation_and_pointer_reset() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data_offline(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );
    settle();

    let nav = Nav::get(&ui);
    let navigations: Arc<Mutex<Vec<(Route, String)>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let navigations = Arc::clone(&navigations);
        nav.on_navigate(move |route, param| {
            navigations.lock().unwrap().push((route, param.to_string()));
        });
    }
    let toggles = Arc::new(Mutex::new(0usize));
    {
        let toggles = Arc::clone(&toggles);
        Player::get(&ui).on_toggle_play(move || {
            *toggles.lock().unwrap() += 1;
        });
    }
    let last_nav = |navigations: &Arc<Mutex<Vec<(Route, String)>>>| {
        navigations
            .lock()
            .unwrap()
            .last()
            .map(|(route, param)| (*route, param.clone()))
    };

    // —— 0. 未聚焦任何控件：空格 = 播放/暂停（既有行为，回归守护）———
    tap(&ui, " ");
    assert_eq!(*toggles.lock().unwrap(), 1, "空格应触发播放/暂停");

    // —— 1. Tab 按声明序遍历：首个停靠点 = 侧栏「首页」，Enter 激活 ———
    tab(&ui);
    tap(&ui, &special(slint::platform::Key::Return));
    assert_eq!(last_nav(&navigations), Some((Route::Home, String::new())));
    tab(&ui);
    tap(&ui, &special(slint::platform::Key::Return));
    assert_eq!(
        last_nav(&navigations),
        Some((Route::Discover, String::new())),
        "第二次 Tab 应到「发现」"
    );
    shift_tab(&ui);
    tap(&ui, &special(slint::platform::Key::Return));
    assert_eq!(
        last_nav(&navigations),
        Some((Route::Home, String::new())),
        "Shift+Tab 应回到「首页」"
    );

    // —— 2. 聚焦控件上的空格 = 按下该控件，不冒泡成播放/暂停 ———
    let nav_count = navigations.lock().unwrap().len();
    tap(&ui, " ");
    assert_eq!(
        navigations.lock().unwrap().len(),
        nav_count + 1,
        "空格应激活聚焦的「首页」"
    );
    assert_eq!(
        *toggles.lock().unwrap(),
        1,
        "聚焦控件时空格不得触发播放/暂停"
    );

    // —— 3. 指针活动 → 控件让出焦点 → 空格回到播放/暂停 ———
    hmp_desktop::keynav::pointer_activity(&ui);
    settle();
    tap(&ui, " ");
    assert_eq!(*toggles.lock().unwrap(), 2, "指针活动后空格应回到播放/暂停");
    assert_eq!(
        navigations.lock().unwrap().len(),
        nav_count + 1,
        "焦点已让出，空格不得再激活旧控件"
    );

    // —— 4. 鼠标点击不聚焦也不被吞：键盘聚焦「首页」后首次点击「搜索」即生效 ———
    tab(&ui);
    click(&ui, 120.0, 244.0);
    assert_eq!(
        last_nav(&navigations),
        Some((Route::Search, String::new())),
        "首次点击应直接生效（不被焦点层吞掉）"
    );

    // Data 占位：确认 global 可达（防止未来导出表回归时测试静默失义）
    assert_eq!(Data::get(&ui).get_search_state(), 0);
}
