//! ksni 托盘（Linux/BSD；feature `tray`）。
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
            "Pause"
        } else {
            "Play"
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
                label: "Previous".into(),
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
                label: "Next".into(),
                icon_name: "media-skip-forward".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.command_tx.send(Request::Command(PlayerCommand::Next));
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Stop".into(),
                icon_name: "media-playback-stop".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.command_tx.send(Request::Command(PlayerCommand::Stop));
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
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
        "Hutao Music Player".into()
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

/// 托盘守卫：Drop 时关停服务线程并取消状态订阅任务。
///
/// `serve.rs` 持有到进程收尾；守卫 drop 先于进程退出即优雅关闭
/// （释放 D-Bus 名称、移除图标）。
pub struct Tray {
    handle: ksni::Handle<HmpTray>,
    forward_task: tokio::task::JoinHandle<()>,
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.handle.shutdown();
        self.forward_task.abort();
    }
}

/// 启动 tray（无 session bus 时返回 None，不 panic）。
pub fn spawn_tray(handle: &EngineHandle) -> Option<Tray> {
    let tray = HmpTray::new(handle.command_tx.clone());
    let service = ksni::TrayService::new(tray);
    let tray_handle = service.handle();

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
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        Ok(Some(msg)) => {
            tracing::warn!(%msg, "tray failed to start (no desktop session?); skipping");
            return None;
        }
        Ok(None) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            tracing::warn!("tray service exited during startup (no desktop session?); skipping");
            return None;
        }
    }

    // 输出订阅：DaemonState → 播放标志 → 图标/菜单标签切换（spec：输出仅用于
    // 图标切换）。仅在启动成功后 spawn，失败路径不留孤儿任务。
    let forward_handle = tray_handle.clone();
    let mut state_rx = handle.state_rx.clone();
    let forward_task = tokio::spawn(async move {
        while state_rx.changed().await.is_ok() {
            let playing = matches!(state_rx.borrow().playback.status, PlaybackStatus::Playing);
            forward_handle.update(|t| t.set_playing(playing));
        }
    });

    Some(Tray {
        handle: tray_handle,
        forward_task,
    })
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
