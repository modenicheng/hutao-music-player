//! 系统托盘（spec §4.2 `tray.rs`；feature `tray`）。
//!
//! 最小菜单：播放/暂停、上一首、下一首、停止、退出。
//! 适配器：输入走命令通道（`Request::Command`/`Request::Quit`），
//! 输出订阅状态（仅用于图标/菜单标签切换）。
//!
//! ksni 0.2 API 说明（与 plan 中的 0.3 风格 builder 写法不同）：
//! - `StandardItem` 无 `new/with_update/activate` builder，用公开字段结构体字面量构造，
//!   `activate` 为 `Box<dyn Fn(&mut T)>`（非 `FnMut`）；
//! - `TrayService::spawn()` 消费 self 且失败时在线程内 panic（无 Result）；
//!   这里改为自管线程运行 `TrayService::run()`，用 100ms 启动窗口探测失败
//!   （无 session bus 等会快速返回 `Err`），从而"返回 None 并 warn，不 panic"。

use std::sync::atomic::{AtomicBool, Ordering};

use hmp_core::{PlaybackStatus, PlayerCommand, Request};
use tokio::sync::mpsc;

use crate::engine::EngineHandle;

/// ksni tray 实现。
pub struct HmpTray {
    command_tx: mpsc::UnboundedSender<Request>,
    playing: AtomicBool,
}

impl HmpTray {
    fn new(command_tx: mpsc::UnboundedSender<Request>) -> Self {
        Self {
            command_tx,
            playing: AtomicBool::new(false),
        }
    }

    /// 播放标志（由状态订阅任务更新；仅用于图标/菜单标签切换）。
    fn set_playing(&self, playing: bool) {
        self.playing.store(playing, Ordering::Relaxed);
    }

    fn menu_items(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        let play_label = if self.playing.load(Ordering::Relaxed) {
            "暂停"
        } else {
            "播放"
        };
        vec![
            StandardItem {
                label: play_label.into(),
                icon_name: "media-playback-pause".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this
                        .command_tx
                        .send(Request::Command(PlayerCommand::TogglePlay));
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "上一首".into(),
                icon_name: "media-skip-backward".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this
                        .command_tx
                        .send(Request::Command(PlayerCommand::Previous));
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "下一首".into(),
                icon_name: "media-skip-forward".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.command_tx.send(Request::Command(PlayerCommand::Next));
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "停止".into(),
                icon_name: "media-playback-stop".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.command_tx.send(Request::Command(PlayerCommand::Stop));
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "退出".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.command_tx.send(Request::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

impl ksni::Tray for HmpTray {
    fn id(&self) -> String {
        "hmp".into()
    }
    fn title(&self) -> String {
        "胡桃音乐播放器".into()
    }
    fn icon_name(&self) -> String {
        if self.playing.load(Ordering::Relaxed) {
            "media-playback-pause".into()
        } else {
            "media-playback-start".into()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.menu_items()
    }
}

/// 启动 tray（无 session bus 时返回 None，不 panic）。
///
/// 返回 `ksni::Handle<HmpTray>`：服务线程由本函数自管（ksni 0.2 的
/// `TrayService::spawn()` 消费 self 且失败会在线程内 panic，无法捕获）。
/// 调用方持有 Handle 可 `update()` 图标、`shutdown()` 优雅关停。
pub fn spawn_tray(handle: &EngineHandle) -> Option<ksni::Handle<HmpTray>> {
    let tray = HmpTray::new(handle.command_tx.clone());
    let service = ksni::TrayService::new(tray);
    let tray_handle = service.handle();

    // 输出订阅：DaemonState → 播放标志 → 图标/菜单标签切换（spec：输出仅用于图标切换）。
    {
        let tray_handle = tray_handle.clone();
        let mut state_rx = handle.state_rx.clone();
        tokio::spawn(async move {
            while state_rx.changed().await.is_ok() {
                let playing = matches!(state_rx.borrow().playback.status, PlaybackStatus::Playing);
                tray_handle.update(|t| t.set_playing(playing));
            }
        });
    }

    // 启动探测：自管线程运行服务循环；启动失败（无 session bus）会在 100ms
    // 窗口内返回 Err，成功则阻塞于循环。通过通道区分两种结果，避免 panic。
    let (setup_tx, setup_rx) = std::sync::mpsc::channel::<Option<String>>();
    std::thread::spawn(move || match service.run() {
        Ok(()) => {
            // 仅 `Handle::shutdown()` 时返回；启动窗口内不会发生。
            let _ = setup_tx.send(None);
        }
        Err(e) => {
            let _ = setup_tx.send(Some(e.to_string()));
        }
    });
    match setup_rx.recv_timeout(std::time::Duration::from_millis(100)) {
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Some(tray_handle),
        Ok(Some(msg)) => {
            tracing::warn!(%msg, "tray 启动失败（可能无桌面会话），跳过");
            None
        }
        Ok(None) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            tracing::warn!("tray 服务在启动窗口内退出（可能无桌面会话），跳过");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小菜单须为 5 项：播放/暂停、上一首、下一首、停止、退出。
    #[test]
    fn menu_has_five_entries() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = HmpTray::new(tx);
        let items = tray.menu_items();
        assert_eq!(items.len(), 5);
    }
}
