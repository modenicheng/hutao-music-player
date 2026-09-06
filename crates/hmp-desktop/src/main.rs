//! HMP 桌面应用入口：Slint UI（通透胡桃木设计语言）+ 模拟播放桥。
//! mock-first 阶段：页面数据全部来自 src/mock.rs，播放由 PlayerHost 模拟；
//! 真实后端接线（AppCore/daemon）见 PORTING.md M8。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global};

use hmp_desktop::{bridge, player_host, prefs};

fn main() -> Result<(), slint::PlatformError> {
    let prefs = Arc::new(Mutex::new(prefs::load()));

    let ui = hmp_desktop::AppWindow::new()?;

    {
        let loaded = *prefs.lock().expect("prefs");
        hmp_desktop::Theme::get(&ui).set_mode(loaded.theme_mode);
        hmp_desktop::Quality::get(&ui).set_selected(loaded.quality);
    }

    bridge::load_data(&ui);
    bridge::bind(&ui, Arc::clone(&prefs));
    // Timer 生命周期挂到事件循环：host 保持到 run 结束
    let _player_host = player_host::PlayerHost::bind(&ui, Arc::clone(&prefs));

    ui.run()
}
