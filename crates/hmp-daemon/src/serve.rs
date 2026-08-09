//! `hmp serve` 入口（spec §4.2 `serve.rs`）。
//!
//! 组装 daemon（引擎 + GStreamer 驱动 + QQ 解析器）并接入 Task 3 的
//! Unix socket 控制服务器；SIGINT/SIGTERM → 引擎 Quit → 引擎退出
//! （sticky watch）→ 停服务器 → 清理 socket 后退出。
//! 单实例由 flock 锁文件保证（final review Finding 6）。

use std::path::PathBuf;

use crate::daemon::{Daemon, DaemonConfig};
use crate::server;

/// 前台运行（调试；Ctrl+C 优雅退出）。也是后台 detached 子进程的 daemon 循环。
/// `sink`：GStreamer 输出元素名（`--sink` 命令行覆盖 config.toml；None = 配置/默认）。
pub async fn run_foreground(sink: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    run_inner(DaemonConfig {
        audio_sink: sink.map(|s| s.to_string()),
    })
    .await
}

/// 后台运行：`setsid` 完全脱离当前会话启动子进程（无控制终端、丢弃 stdio），
/// 子进程运行前台 daemon 循环；本函数随即返回（final review Finding 8）。
/// `sink` 经命令行 `--sink NAME` 透传给子进程（打磨）。
pub async fn run_background(sink: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    spawn_detached(&background_args(sink))?;
    Ok(())
}

/// 以 `setsid`（util-linux 外部命令）脱离会话启动本可执行文件。
///
/// 单一 detach 点：auto-spawn（CLI）与 `serve --background` 都经此脱离
/// 控制终端/进程组；stdio 置空避免后端输出干扰调用方终端。
pub fn spawn_detached(args: &[&str]) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new("setsid")
        .arg(&exe)
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
    let mut args = vec!["serve"];
    if let Some(s) = sink {
        args.push("--sink");
        args.push(s);
    }
    args
}

async fn run_inner(cfg: DaemonConfig) -> Result<(), Box<dyn std::error::Error>> {
    // 里程碑 G：输出设备来自 config.toml `[audio] sink`（显式注入优先；无段 → 系统默认）。
    let audio = hmp_storage::Config::load().audio;
    let cfg = DaemonConfig {
        audio_sink: merge_audio_sink(cfg.audio_sink.as_deref(), audio.sink),
    };
    let path = server::socket_path();
    // 父目录（XDG_RUNTIME_DIR 已存在；/tmp/hmp-{uid} 回退目录须创建且仅属本用户，
    // final review Finding 5）。
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    // 单实例：先于任何 socket 操作获取 flock（final review Finding 6）。
    // 只有持锁者才进入 stale-socket 清理/绑定流程；锁文件留在原地（flock
    // 随进程死亡自动释放，残留文件无害——flock 才是真正的守卫）。
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    {
        use std::os::unix::io::AsRawFd;
        if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            eprintln!("已有后端在运行，退出");
            return Ok(());
        }
    }
    let daemon = Daemon::start(cfg)?;
    // 清理残留（上次异常退出可能留下）；持锁者才执行，无 TOCTOU 竞争。
    if path.exists() {
        // 尝试连接：能连说明有活 daemon，本实例退出；不能连则删残留
        if tokio::net::UnixStream::connect(&path).await.is_ok() {
            eprintln!("已有后端在运行，退出");
            return Ok(());
        }
        let _ = std::fs::remove_file(&path);
    }
    let listener = tokio::net::UnixListener::bind(&path)?;
    // 强制 socket 0600（trust boundary，final review Finding 5）；失败仅告警不中止。
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!(%e, "设置 socket 0600 失败");
        }
    }
    tracing::info!(?path, "后端已就绪");
    // 优雅退出：SIGINT/SIGTERM → 只发 Request::Quit（引擎处理完 Quit 才退出
    // 并置位 terminated；不再有并行的 quit_tx，避免清理先于 driver.shutdown）。
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
        });
    }
    let server_handle = tokio::spawn(server::serve(listener, handle.clone()));
    // 桌面集成（spec §4.2）：系统托盘 + MPRIS（feature 门控；无会话时跳过不 panic）。
    #[cfg(feature = "tray")]
    let tray = crate::tray::spawn_tray(&handle);
    #[cfg(feature = "mpris")]
    let mpris = crate::mpris::start_mpris(
        handle.command_tx.clone(),
        handle.state_rx.clone(),
        handle.caps_rx.clone(),
    )
    .await;
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
    // 停服务器（监听关闭）+ 清理 + 关 tray / 释放 MPRIS bus 名（优雅退出，spec §6）。
    server_handle.abort();
    let _ = tokio::fs::remove_file(&path).await;
    #[cfg(feature = "tray")]
    if let Some(tray) = tray {
        tray.shutdown();
    }
    #[cfg(feature = "mpris")]
    drop(mpris);
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
        assert_eq!(background_args(None), vec!["serve"]);
        assert_eq!(
            background_args(Some("fakesink")),
            vec!["serve", "--sink", "fakesink"]
        );
    }
}
