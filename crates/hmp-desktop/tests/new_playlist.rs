//! 新建歌单命名弹层集成测试（testing backend + 合成按键，单测试单进程）。
//!
//! 守护「新建歌单」入口（ui/new-playlist.slint，侧栏「+」）：
//! 1. 「+」是键盘可达的（此前是 mock 期遗留的无动作按钮），Enter 打开命名弹层；
//! 2. 弹层打开即聚焦输入框，直接打字进入；
//! 3. 空名回车不发请求；输入名称后回车发出 `Data.create-playlist(原样名称)`
//!    （去空白、daemon 请求与回包编排在 menu_bridge，另有单测）。
//!
//! 到达「+」：从窗口根作用域 Tab 正向走——主导航 9 项 → 「自建歌单」分组钮
//! → 「+」，共 11 次（不依赖真实媒体库的歌单数量：歌单行排在其后）。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, SharedString, platform::WindowEvent};

use hmp_desktop::Data;

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

#[test]
fn new_playlist_dialog_opens_from_keyboard_and_submits() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data_offline(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );
    settle(320);

    let creates: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let creates = Arc::clone(&creates);
        Data::get(&ui)
            .on_create_playlist(move |name| creates.lock().unwrap().push(name.to_string()));
    }

    for _ in 0..11 {
        tap(&ui, slint::platform::Key::Tab);
    }
    tap(&ui, slint::platform::Key::Return);
    settle(64);

    // 空名回车：不发请求
    tap(&ui, slint::platform::Key::Return);
    assert!(creates.lock().unwrap().is_empty(), "空名不得发出新建请求");

    // 弹层打开即聚焦输入框：打字 + 回车
    type_text(&ui, "夜路");
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *creates.lock().unwrap(),
        vec!["夜路".to_string()],
        "输入名称后回车应发出新建请求"
    );
}
