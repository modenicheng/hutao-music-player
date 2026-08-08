//! `hmp serve` 入口（spec §4.2 `serve.rs`）。
//!
//! 组装 daemon（引擎 + GStreamer 驱动 + QQ 解析器）并接入 Task 3 的
//! Unix socket 控制服务器；SIGINT/SIGTERM → 引擎 Quit → 停服务器 →
//! 清理 socket 后退出。Task 6 将在此追加 tray/MPRIS 启动（feature 门控）。

use crate::daemon::{Daemon, DaemonConfig};
use crate::server;

/// 前台运行（调试；Ctrl+C 优雅退出）。
pub async fn run_foreground() -> Result<(), Box<dyn std::error::Error>> {
    run_inner(DaemonConfig { audio_sink: None }).await
}

/// 后台运行（CLI 拉起；detached）。
pub async fn run_background() -> Result<(), Box<dyn std::error::Error>> {
    run_inner(DaemonConfig { audio_sink: None }).await
}

async fn run_inner(cfg: DaemonConfig) -> Result<(), Box<dyn std::error::Error>> {
    let daemon = Daemon::start(cfg)?;
    let path = server::socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 清理残留（上次异常退出可能留下）
    if path.exists() {
        // 尝试连接：能连说明有活 daemon，本实例退出；不能连则删残留
        if tokio::net::UnixStream::connect(&path).await.is_ok() {
            eprintln!("已有后端在运行，退出");
            return Ok(());
        }
        let _ = std::fs::remove_file(&path);
    }
    let listener = tokio::net::UnixListener::bind(&path)?;
    tracing::info!(?path, "后端已就绪");
    // 优雅退出：SIGINT/SIGTERM → 发 Quit
    let (quit_tx, mut quit_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let handle = daemon.handle;
    {
        let handle = handle.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let mut sigint = signal(SignalKind::interrupt()).unwrap();
                let mut sigterm = signal(SignalKind::terminate()).unwrap();
                tokio::select! {
                    _ = sigint.recv() => {}
                    _ = sigterm.recv() => {}
                }
            }
            let _ = handle.command_tx.send(hmp_core::Request::Quit);
            let _ = quit_tx.send(());
        });
    }
    let server_handle = tokio::spawn(server::serve(listener, handle.clone()));
    // 等待退出信号
    let _ = quit_rx.recv().await;
    // 停服务器（监听关闭）+ 清理
    server_handle.abort();
    let _ = tokio::fs::remove_file(&path).await;
    tracing::info!("后端已退出");
    Ok(())
}
