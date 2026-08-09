//! MPRIS 适配（spec §4.2 `mpris.rs`；feature `mpris`）。
//!
//! 复用现有 hmp-mpris：它已消费（命令通道, 状态 watch）两个接口。
//! daemon 持有返回的 `MprisService` 防 Drop（Drop 释放 bus 名）。

use hmp_core::{DaemonState, PlaybackCapabilities, PlaybackState, Request};
use tokio::sync::{mpsc, watch};

/// 启动 MPRIS（bus 名冲突/无总线时返回 None）。
///
/// 适配两层通道：hmp-mpris 消费 `PlayerCommand`（反向转发为
/// `Request::Command`）与 `PlaybackState`（从 `DaemonState.playback` 投影）；
/// 播放能力（CanGoNext/CanGoPrevious）直接转发引擎发布的 caps watch
/// （final review Finding 9）。
pub async fn start_mpris(
    command_tx: mpsc::UnboundedSender<Request>,
    state_rx: watch::Receiver<DaemonState>,
    caps_rx: watch::Receiver<PlaybackCapabilities>,
) -> Option<hmp_mpris::MprisService> {
    // hmp-mpris 的 MprisService::start_with_capabilities 需要 (UnboundedSender<PlayerCommand>,
    // watch::Receiver<PlaybackState>, Option<watch::Receiver<PlaybackCapabilities>>)；
    // 适配：daemon 状态 → playback 子集；命令反向转换。
    let cmd_tx = {
        let (tx, mut rx) = mpsc::unbounded_channel::<hmp_core::PlayerCommand>();
        let daemon_tx = command_tx.clone();
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                let _ = daemon_tx.send(Request::Command(cmd));
            }
        });
        tx
    };
    let playback_rx = {
        let (tx, rx) = watch::channel::<PlaybackState>(state_rx.borrow().playback.clone());
        let state_rx = state_rx.clone();
        tokio::spawn(async move {
            let mut state_rx = state_rx;
            while state_rx.changed().await.is_ok() {
                let _ = tx.send(state_rx.borrow().playback.clone());
            }
        });
        rx
    };
    // OpenUri 转发：MPRIS → 引擎 `Request::OpenUri`（file:// 本地播放，C4）。
    let open_uri_tx = {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let daemon_tx = command_tx.clone();
        tokio::spawn(async move {
            while let Some(uri) = rx.recv().await {
                let _ = daemon_tx.send(Request::OpenUri(uri));
            }
        });
        tx
    };
    // Quit 转发：MPRIS 根接口 Quit() → 引擎 `Request::Quit`（hmp quit 同路径）。
    let quit_tx = {
        let (tx, mut rx) = mpsc::unbounded_channel::<()>();
        let daemon_tx = command_tx.clone();
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                let _ = daemon_tx.send(Request::Quit);
            }
        });
        tx
    };
    match hmp_mpris::MprisService::start_with_capabilities_uri_and_quit(
        cmd_tx,
        playback_rx,
        Some(caps_rx),
        Some(open_uri_tx),
        Some(quit_tx),
    )
    .await
    {
        Ok(service) => Some(service),
        Err(e) => {
            tracing::warn!(%e, "MPRIS 启动失败，跳过");
            None
        }
    }
}
