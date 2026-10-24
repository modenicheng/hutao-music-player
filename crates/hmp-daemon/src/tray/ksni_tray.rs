//! ksni 托盘（Linux/BSD；feature `tray`）。
//!
//! 最小菜单：播放/暂停、上一首、下一首、停止、退出。
//! 适配器：输入走命令通道（`Request::Command`/`Request::Quit`），
//! 输出订阅状态（仅用于图标/菜单标签切换）。
//!
//! ksni 0.3 blocking API 说明（S12：从 0.2 的 `TrayService` + 自管线程迁移；
//! tokio/async-io feature 与 keyring 链的 zbus 5 feature unification 互斥，
//! 详见 Cargo.toml 注释）：
//! - `blocking` 模块经 `TrayMethods::spawn()` 返回 `Result<Handle>`——启动
//!   失败（无 session bus）直接 `Err`，不再需要 0.2 时代的自管线程 +
//!   100ms 启动窗口探测；
//! - `Handle::update` 内部 `block_on`（ksni 自建 current_thread runtime），
//!   状态订阅任务（tokio）里调用安全（同步短临界区，非 async 上下文持有）；
//! - `Handle::shutdown()` 返回 `ShutdownAwaiter`，Drop 时只发起不等完成
//!   （进程收尾，等待由内核回收兜底）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use hmp_core::{PlaybackStatus, PlayerCommand, Request};
use ksni::blocking::TrayMethods;
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

/// 托盘守卫：Drop 时关停服务并取消状态订阅任务。
///
/// `serve.rs` 持有到进程收尾；守卫 drop 先于进程退出即优雅关闭
/// （释放 D-Bus 名称、移除图标）。
pub struct Tray {
    handle: ksni::blocking::Handle<HmpTray>,
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
    // blocking spawn：启动失败（无 session bus 等）直接 Err——0.2 时代的
    // 自管线程 + 100ms 探测窗口不再需要。
    let tray_handle = Arc::new(tray.spawn().ok()?);

    // 输出订阅：DaemonState → 播放标志 → 图标/菜单标签切换（spec：输出仅用于
    // 图标切换）。仅在启动成功后 spawn，失败路径不留孤儿任务。
    let forward_handle = tray_handle.clone();
    let mut state_rx = handle.state_rx.clone();
    let forward_task = tokio::spawn(async move {
        while state_rx.changed().await.is_ok() {
            let playing = matches!(state_rx.borrow().playback.status, PlaybackStatus::Playing);
            // Handle 自身内部同步（Arc 包装便于跨 move 共享）；update 内部
            // block_on 短临界区，在 tokio 任务里同步调用可接受（非持锁跨 await）。
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
