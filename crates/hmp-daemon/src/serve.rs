//! Cross-platform daemon process orchestration.

use std::time::Duration;

use hmp_control::{FrontendLeaseTracker, LifecycleMode};

use crate::daemon::{Daemon, DaemonConfig};
use crate::server;

/// 前台运行（调试；Ctrl+C 优雅退出）。也是后台 detached 子进程的 daemon 循环。
/// `sink`：GStreamer 输出元素名（`--sink` 命令行覆盖 config.toml；None = 配置/默认）。
pub async fn run_foreground(sink: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    run(sink, LifecycleMode::Autonomous).await
}

/// Run a daemon owned by a desktop frontend lease.
pub async fn run_frontend_owned(sink: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    run(
        sink,
        LifecycleMode::FrontendOwned {
            orphan_grace: Duration::from_secs(30),
        },
    )
    .await
}

/// 后台运行：`setsid` 完全脱离当前会话启动子进程（无控制终端、丢弃 stdio），
/// 子进程运行前台 daemon 循环；本函数随即返回（final review Finding 8）。
/// `sink` 经命令行 `--sink NAME` 透传给子进程（打磨）。
pub async fn run_background(sink: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    spawn_detached(&background_args(sink))?;
    Ok(())
}

/// Start the sibling `hmpd` executable detached from the invoking controller.
pub fn spawn_detached(args: &[&str]) -> std::io::Result<()> {
    let current = std::env::current_exe()?;
    let exe = current.with_file_name(if cfg!(windows) { "hmpd.exe" } else { "hmpd" });
    #[cfg(unix)]
    let mut command = {
        let mut command = std::process::Command::new("setsid");
        command.arg(&exe);
        command
    };
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = std::process::Command::new(&exe);
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        command
    };
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// 合并输出设备：显式注入优先，否则用 config.toml `[audio] sink`（无 → None）。
/// 里程碑 G：输出设备选择（`config.toml [audio] sink` → GstDriver）。
fn merge_audio_sink(injected: Option<&str>, configured: Option<String>) -> Option<String> {
    injected.map(|s| s.to_string()).or(configured)
}

/// 打磨：`serve --background` 的子进程参数（含 `--sink NAME` 时透传）。
fn background_args(sink: Option<&str>) -> Vec<&str> {
    let mut args = vec!["--autonomous"];
    if let Some(s) = sink {
        args.push("--sink");
        args.push(s);
    }
    args
}

pub async fn run(
    sink: Option<&str>,
    mode: LifecycleMode,
) -> Result<(), Box<dyn std::error::Error>> {
    // 里程碑 G：输出设备来自 config.toml `[audio] sink`（显式注入优先；无段 → 系统默认）。
    let audio = hmp_storage::Config::load().audio;
    let cfg = DaemonConfig {
        audio_sink: merge_audio_sink(sink, audio.sink),
    };
    // Binding is the single-instance gate on both platforms and happens before
    // GStreamer/database initialization.
    let listener = hmp_control::transport::Listener::bind().await?;
    let daemon = Daemon::start(cfg)?;
    tracing::info!(endpoint = ?hmp_control::transport::endpoint(), "后端已就绪");
    // 优雅退出：SIGINT/SIGTERM → 只发 Request::Quit（引擎处理完 Quit 才退出
    // 并置位 terminated；不再有并行的 quit_tx，避免清理先于 driver.shutdown）。
    let handle = daemon.handle.clone();
    {
        let handle = handle.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                let _ = handle.command_tx.send(hmp_core::Request::Quit);
            }
        });
    }
    let lifecycle = FrontendLeaseTracker::new(mode, handle.command_tx.clone());
    let server_handle = tokio::spawn(server::serve_with_lifecycle(
        listener,
        handle.clone(),
        lifecycle,
    ));
    // 等待引擎实际终止（sticky watch；`hmp quit` / tray 退出 / SIGINT/SIGTERM 均
    // 收敛到引擎处理 Request::Quit 后置位，final review Finding 7）。信号任务只发
    // Quit，不再旁路通知，故此处仅需等引擎退出，清理必然在 driver.shutdown 之后。
    let term_wait = async {
        let mut term = handle.terminated.clone();
        if *term.borrow() {
            return;
        }
        let _ = term.changed().await;
    };
    term_wait.await;
    // Dropping the server future closes the platform listener and instance guard.
    server_handle.abort();
    tracing::info!("后端已退出");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 里程碑 G：输出设备合并——显式注入优先于配置；配置缺失 → None。
    #[test]
    fn audio_sink_config_merges_with_injection() {
        assert_eq!(
            merge_audio_sink(Some("injected"), Some("configed".into())),
            Some("injected".to_string())
        );
        assert_eq!(
            merge_audio_sink(None, Some("configed".into())),
            Some("configed".to_string())
        );
        assert_eq!(merge_audio_sink(None, None), None);
        assert_eq!(
            merge_audio_sink(Some("injected"), None),
            Some("injected".to_string())
        );
    }

    /// 打磨：`--sink` 命令行参数 → detached 子进程参数透传。
    #[test]
    fn background_args_include_sink_when_given() {
        assert_eq!(background_args(None), vec!["--autonomous"]);
        assert_eq!(
            background_args(Some("fakesink")),
            vec!["--autonomous", "--sink", "fakesink"]
        );
    }
}
