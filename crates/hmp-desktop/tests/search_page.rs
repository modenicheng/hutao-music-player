//! 搜索页交互集成测试（testing backend + 合成按键，单测试单进程）。
//!
//! 守护 ui/search-page.slint：
//! 1. 进入页面自动聚焦搜索框（DESIGN §3.2）：直接打字即进入输入框；
//! 2. 新查询回到「歌曲」tab（此前停在上次选的「歌手」tab 上，新结果没有歌手
//!    时一进来就是「该分类下没有相关结果」）；
//! 3. 清空键：一键清掉关键词与旧结果、焦点回到输入框；
//! 4. Ctrl+F 把焦点送回搜索框（焦点已在输入框时请求幂等）。
//!
//! 导航与搜索回调由本测试自接（只记录 + 回写路由），不出网。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, SharedString, platform::WindowEvent};

use hmp_desktop::{Data, KeyNav, Nav, Route};

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

fn tap(ui: &hmp_desktop::AppWindow, k: slint::platform::Key) {
    let k: SharedString = k.into();
    let k = k.as_str().to_owned();
    key(ui, &k, true);
    key(ui, &k, false);
}

fn type_text(ui: &hmp_desktop::AppWindow, text: &str) {
    for ch in text.chars() {
        let ch = ch.to_string();
        key(ui, &ch, true);
        key(ui, &ch, false);
    }
}

fn ctrl_f(ui: &hmp_desktop::AppWindow) {
    let control: SharedString = slint::platform::Key::Control.into();
    let control = control.as_str().to_owned();
    key(ui, &control, true);
    key(ui, "f", true);
    key(ui, "f", false);
    key(ui, &control, false);
}

#[test]
fn search_page_focus_clear_and_tab_reset() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data_offline(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );
    settle(320);

    let nav = Nav::get(&ui);
    {
        let ui_weak = ui.as_weak();
        nav.on_navigate(move |route, param| {
            if let Some(ui) = ui_weak.upgrade() {
                let nav = Nav::get(&ui);
                nav.set_route(route);
                nav.set_param(param);
            }
        });
    }
    let searches: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let searches = Arc::clone(&searches);
        Data::get(&ui).on_search(move |query| searches.lock().unwrap().push(query.to_string()));
    }

    // —— 1. 进页自动聚焦：直接打字进输入框 ——
    nav.invoke_navigate(Route::Search, "".into());
    settle(64);
    type_text(&ui, "周杰伦");
    assert_eq!(
        Data::get(&ui).get_search_query(),
        "周杰伦",
        "进页后应可直接输入"
    );

    // —— 2. 新查询回到「歌曲」tab ——
    let data = Data::get(&ui);
    data.set_search_tab(2);
    data.set_search_state(2);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *searches.lock().unwrap(),
        vec!["周杰伦".to_string()],
        "回车提交搜索"
    );
    assert_eq!(data.get_search_tab(), 0, "新查询应回到「歌曲」tab");

    // —— 3. 清空键：输入框之后的下一个停靠点 ——
    tap(&ui, slint::platform::Key::Tab);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(data.get_search_query(), "", "清空键应清掉关键词");
    assert_eq!(data.get_search_state(), 0, "清空键应回到初始态");
    // 焦点回到输入框：直接打字
    type_text(&ui, "林");
    assert_eq!(data.get_search_query(), "林", "清空后焦点应回到输入框");

    // —— 4. Ctrl+F 把焦点送回搜索框 ——
    // 焦点已在输入框：请求是幂等的（输入框不吞 Ctrl+F，会再请求一次并原地聚焦）
    let seq_before = KeyNav::get(&ui).get_focus_search_seq();
    ctrl_f(&ui);
    settle(32);
    assert!(
        KeyNav::get(&ui).get_focus_search_seq() > seq_before,
        "Ctrl+F 应登记聚焦请求"
    );
    let seq_before = KeyNav::get(&ui).get_focus_search_seq();
    type_text(&ui, "忆");
    assert_eq!(data.get_search_query(), "林忆", "Ctrl+F 后焦点仍在输入框");
    // 退掉刚打的字，保持下面的断言简单
    tap(&ui, slint::platform::Key::Backspace);
    assert_eq!(data.get_search_query(), "林");
    // 把焦点移到别处：Tab ×2（清空键 → 搜索钮；关键词非空、搜索钮可用）→ Enter 提交
    tap(&ui, slint::platform::Key::Tab);
    tap(&ui, slint::platform::Key::Tab);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        searches.lock().unwrap().last(),
        Some(&"林".to_string()),
        "搜索钮可键盘到达"
    );
    // 此刻焦点在搜索钮上：Ctrl+F 应回到输入框
    ctrl_f(&ui);
    settle(32);
    assert_eq!(
        KeyNav::get(&ui).get_focus_search_seq(),
        seq_before + 1,
        "焦点不在输入框时 Ctrl+F 应请求聚焦"
    );
    type_text(&ui, "夕");
    assert_eq!(data.get_search_query(), "林夕", "Ctrl+F 后打字应进入输入框");
}
