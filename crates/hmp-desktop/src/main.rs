//! HMP 桌面应用入口：Slint UI（通透胡桃木设计语言）+ daemon 播放桥（M8）。
//! UI 是 daemon 的又一个适配器（docs/PROJECT.md §8.6）：IPC 传输在 backend.rs，
//! 状态映射在 player_bridge.rs；库页数据经 library_view 直读媒体库（离线降级空态）。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global};

use hmp_desktop::{backend, bridge, player_bridge, prefs};

fn main() -> Result<(), slint::PlatformError> {
    let prefs = Arc::new(Mutex::new(prefs::load()));

    // IPC runtime（bootstrap 第一步）：订阅任务挂上后随 main 存活到 run() 退出。
    let runtime = Arc::new(backend::BackendRuntime::new().expect("tokio runtime"));

    let ui = hmp_desktop::AppWindow::new()?;

    {
        let loaded = *prefs.lock().expect("prefs");
        hmp_desktop::Theme::get(&ui).set_mode(loaded.theme_mode);
        hmp_desktop::Quality::get(&ui).set_selected(loaded.quality);
    }

    bridge::load_data(&ui);
    bridge::bind(&ui, Arc::clone(&prefs), Arc::clone(&runtime));
    // 播放桥：连接/拉起 daemon → 订阅状态推送；彻底失败降级离线
    // （Player 全空、命令 no-op），UI 照常打开。
    player_bridge::bind(&ui, Arc::clone(&runtime), Arc::clone(&prefs));

    ui.run()
}
