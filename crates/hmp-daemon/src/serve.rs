//! Cross-platform daemon process orchestration.

use std::time::Duration;

use hmp_control::{FrontendLeaseTracker, LifecycleMode};

use crate::daemon::{Daemon, DaemonConfig};
use crate::server;

/// Run an autonomous daemon in the foreground.
pub async fn run_foreground() -> Result<(), Box<dyn std::error::Error>> {
    run(LifecycleMode::Autonomous).await
}

/// Run a daemon owned by a desktop frontend lease.
pub async fn run_frontend_owned() -> Result<(), Box<dyn std::error::Error>> {
    run(LifecycleMode::FrontendOwned {
        orphan_grace: Duration::from_secs(30),
    })
    .await
}

/// Start an autonomous sibling `hmpd` detached from the invoking controller.
pub async fn run_background() -> Result<(), Box<dyn std::error::Error>> {
    spawn_detached(&background_args())?;
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

fn background_args() -> Vec<&'static str> {
    vec!["--autonomous"]
}

pub async fn run(mode: LifecycleMode) -> Result<(), Box<dyn std::error::Error>> {
    // Binding is the cross-platform single-instance gate and happens before
    // audio-device/database initialization.
    let listener = hmp_control::transport::Listener::bind().await?;
    let daemon = Daemon::start(DaemonConfig)?;
    tracing::info!(endpoint = ?hmp_control::transport::endpoint(), "后端已就绪");

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

    // System media adapters are state projections only. They share the daemon's
    // command/state/capability channels and never own playback policy or audio.
    #[cfg(all(unix, feature = "mpris"))]
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

    // Every exit path converges on the engine processing Request::Quit.
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
    #[cfg(all(unix, feature = "mpris"))]
    drop(mpris);
    #[cfg(windows)]
    drop(smtc);
    tracing::info!("后端已退出");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_args_are_backend_neutral() {
        assert_eq!(background_args(), vec!["--autonomous"]);
    }
}
