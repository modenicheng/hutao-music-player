//! Windows System Media Transport Controls adapter.
//!
//! This module only translates daemon channels to the backend-neutral channels
//! consumed by `hmp-smtc`. Audio remains owned by `PlaybackDriver`.

use hmp_core::{DaemonState, PlaybackCapabilities, PlaybackState, PlayerCommand, Request};
use tokio::sync::{mpsc, watch};

/// Starts Windows media-key, shell-overlay, and lock-screen integration.
///
/// Windows Runtime initialization is deliberately non-fatal: a restricted or
/// headless session can still run the daemon and accept IPC commands.
pub fn start_smtc(
    command_tx: mpsc::UnboundedSender<Request>,
    state_rx: watch::Receiver<DaemonState>,
    caps_rx: watch::Receiver<PlaybackCapabilities>,
) -> Option<hmp_smtc::SmtcService> {
    let player_command_tx = bridge_commands(command_tx);
    let playback_rx = project_playback(state_rx);
    match hmp_smtc::SmtcService::start(player_command_tx, playback_rx, caps_rx) {
        Ok(service) => Some(service),
        Err(error) => {
            tracing::warn!(%error, "Windows 媒体控制启动失败，跳过");
            None
        }
    }
}

fn bridge_commands(
    daemon_tx: mpsc::UnboundedSender<Request>,
) -> mpsc::UnboundedSender<PlayerCommand> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            if daemon_tx.send(Request::Command(command)).is_err() {
                break;
            }
        }
    });
    tx
}

fn project_playback(mut state_rx: watch::Receiver<DaemonState>) -> watch::Receiver<PlaybackState> {
    let (tx, rx) = watch::channel(state_rx.borrow().playback.clone());
    tokio::spawn(async move {
        while state_rx.changed().await.is_ok() {
            if tx.send(state_rx.borrow().playback.clone()).is_err() {
                break;
            }
        }
    });
    rx
}
