//! `hmp serve` 入口（spec §4.2 `serve.rs`）。
//!
//! 组装顺序：单实例裁决 + 端点绑定（flock / connect 探测 /
//! first_pipe_instance）→ daemon（引擎 + Rust 音频驱动 + QQ 解析器）→
//! Unix socket 控制服务器 + tray/MPRIS/SMTC；SIGINT/SIGTERM → 引擎 Quit
//! → 引擎退出（sticky watch）→ 停服务器 → 清理 socket → 关桌面集成后退出。

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

use crate::daemon::{Daemon, DaemonConfig};
use crate::server;

/// 前台运行（调试；Ctrl+C 优雅退出）。也是后台 detached 子进程的 daemon 循环。
pub async fn run_foreground() -> Result<(), Box<dyn std::error::Error>> {
    run_inner(DaemonConfig).await
}

/// 后台运行：`setsid` 完全脱离当前会话启动子进程（无控制终端、丢弃 stdio），
/// 子进程运行前台 daemon 循环；本函数随即返回（final review Finding 8）。
pub async fn run_background() -> Result<(), Box<dyn std::error::Error>> {
    spawn_detached(&background_args())?;
    Ok(())
}

/// 以 `setsid`（util-linux 外部命令）脱离会话启动本可执行文件。
pub fn spawn_detached(args: &[&str]) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    spawn_detached_exe(&exe, args)
}

/// 脱离会话启动指定可执行文件（单一 detach 点）。
///
/// CLI 以 current_exe（即 `hmp` 自身）调用；桌面端进程是 hmp-desktop，
/// 解析出 `hmp` 二进制路径后也经此拉起后端。stdio 置空避免后端输出
/// 干扰调用方终端。
#[cfg(unix)]
pub fn spawn_detached_exe(exe: &Path, args: &[&str]) -> std::io::Result<()> {
    std::process::Command::new("setsid")
        .arg(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// Windows 脱离会话：无 setsid，用 creation_flags 隐藏控制台 +
/// 独立进程组（Ctrl+C 不沿进程组传播给 daemon）。
#[cfg(windows)]
pub fn spawn_detached_exe(exe: &Path, args: &[&str]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    std::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    Ok(())
}

/// `serve --background` 的子进程参数。
fn background_args() -> Vec<&'static str> {
    vec!["serve"]
}

async fn run_inner(cfg: DaemonConfig) -> Result<(), Box<dyn std::error::Error>> {
    let path = server::socket_path();
    // Unix：父目录须创建且仅属本用户（XDG_RUNTIME_DIR 已存在时跳过创建，
    // final review Finding 5）。Windows 端点是命名管道（\\.\pipe\hmp），
    // 不落盘、无需目录。
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    // 单实例（Unix）：先于任何 socket 操作获取 flock（final review Finding 6）。
    // 只有持锁者才进入 stale-socket 清理/绑定流程；锁文件留在原地（flock
    // 随进程死亡自动释放，残留文件无害——flock 才是真正的守卫）。
    // Windows：无 flock，由 connect 探测 + bind 的 first_pipe_instance
    // （已绑定→ERROR_ACCESS_DENIED）双道裁决。
    #[cfg(unix)]
    {
        let lock_path = PathBuf::from(format!("{}.lock", path.display()));
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;
        {
            use std::os::unix::io::AsRawFd;
            if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                eprintln!("daemon already running; exiting");
                return Ok(());
            }
        }
    }
    // 单例裁决 + 端点绑定（先于 Daemon::start）：Windows 无 flock，第二实例
    // 必须在启动引擎（开库、close_stale_sessions、恢复播放态）之前出局，
    // 否则会干扰运行中 daemon 的数据库与会话。探测能连 → 有活 daemon，退出。
    // Unix 另清理残留 socket 文件（上次异常退出可能留下；持锁者才执行，
    // 无 TOCTOU 竞争）；Windows 管道随进程消失，无残留可清。
    if crate::transport::IpcStream::connect(&path).await.is_ok() {
        eprintln!("daemon already running; exiting");
        return Ok(());
    }
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(&path);
    }
    let listener = match crate::transport::IpcListener::bind(&path) {
        Ok(listener) => listener,
        #[cfg(windows)]
        Err(e) if e.raw_os_error() == Some(5) => {
            // ERROR_ACCESS_DENIED：first_pipe_instance 撞上运行中的 daemon。
            eprintln!("daemon already running; exiting");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    // 强制 socket 0600（trust boundary，final review Finding 5）；失败仅告警不中止。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!(%e, "failed to set socket permissions 0600");
        }
    }
    // 端点就绪后才启动引擎；失败须先撤销已绑定的端点（Unix 残留 socket 文件
    // 会挡住下一次启动的残留探测；Windows 管道随进程消失无需清理）。
    let daemon = match Daemon::start(cfg) {
        Ok(daemon) => daemon,
        Err(e) => {
            #[cfg(unix)]
            let _ = std::fs::remove_file(&path);
            return Err(e.into());
        }
    };
    tracing::info!(?path, "daemon ready");
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
            // Windows 无 SIGTERM；Ctrl+C（含 taskkill 无 /F 的 WM_CLOSE 路径）
            // 经控制台处理器送达。
            #[cfg(windows)]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            let _ = handle.command_tx.send(hmp_core::Request::Quit);
        });
    }
    let server_handle = tokio::spawn(server::serve(listener, handle.clone()));
    // 桌面集成（spec §4.2）：系统托盘 + MPRIS（feature 门控；无会话时跳过不 panic）。
    // 托盘为 RAII 守卫：提前 return / 正常收尾两条路径都经 Drop 优雅关停
    // （移除图标、回收属主线程）。
    #[cfg(feature = "tray")]
    let _tray = crate::tray::spawn_tray(&handle);
    #[cfg(feature = "mpris")]
    let mpris = crate::mpris::start_mpris(
        handle.command_tx.clone(),
        handle.state_rx.clone(),
        handle.caps_rx.clone(),
    )
    .await;
    #[cfg(windows)]
    let smtc = crate::smtc::start_smtc(
        handle.command_tx.clone(),
        handle.state_rx.clone(),
        handle.caps_rx.clone(),
    );
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
    #[cfg(unix)]
    {
        let _ = tokio::fs::remove_file(&path).await;
    }
    #[cfg(feature = "tray")]
    drop(_tray);
    #[cfg(feature = "mpris")]
    drop(mpris);
    #[cfg(windows)]
    drop(smtc);
    tracing::info!("daemon exited");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_args_are_backend_neutral() {
        assert_eq!(background_args(), vec!["serve"]);
    }

    /// spawn_detached_exe 是纯 detach 点：对任意可执行文件路径都能发起
    /// setsid 启动（子进程立即退出；本函数只验证 spawn 不报错）。
    #[test]
    fn spawn_detached_exe_spawns_external_binary() {
        let exe = Path::new("/bin/true");
        if !exe.exists() {
            return; // 环境无 /bin/true 时跳过（setsid 语义已在生产路径覆盖）
        }
        spawn_detached_exe(exe, &[]).expect("setsid 启动外部二进制应成功");
    }
}
