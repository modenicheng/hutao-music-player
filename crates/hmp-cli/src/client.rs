//! CLI → daemon 客户端（spec §4.3 `client.rs`）。

use std::time::Duration;

use hmp_core::{Request, Response};

/// CLI 错误。
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("无法连接后端: {0}")]
    Connect(String),
    #[error("后端响应错误: {code:?} {message}")]
    Response {
        code: hmp_core::IpcErrorCode,
        message: String,
    },
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("io 错误: {0}")]
    Io(#[from] std::io::Error),
}

/// 与后端的一条连接。
pub struct DaemonClient {
    inner: hmp_control::ControlClient,
}

impl DaemonClient {
    /// 连接或拉起 autonomous `hmpd`，随后轮询平台控制端点。
    pub async fn connect_or_spawn() -> Result<Self, CliError> {
        match Self::try_connect().await {
            Ok(c) => return Ok(c),
            Err(CliError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                spawn_daemon()?;
                return wait_for_daemon(Duration::from_secs(3)).await;
            }
            Err(CliError::Io(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                spawn_daemon()?;
                return wait_for_daemon(Duration::from_secs(3)).await;
            }
            Err(e) => return Err(e),
        }
    }

    async fn try_connect() -> Result<Self, CliError> {
        let inner = hmp_control::ControlClient::connect()
            .await
            .map_err(map_control_error)?;
        Ok(Self { inner })
    }

    /// 发请求并收响应。
    pub async fn request(&mut self, req: &Request) -> Result<Response, CliError> {
        self.inner
            .request(req.clone())
            .await
            .map_err(map_control_error)
    }
}

/// 经 `hmp_daemon::serve::spawn_detached` 拉起同目录的 `hmpd --autonomous`。
/// Linux 使用 `setsid`；Windows 使用无窗口的新进程组；两端都丢弃 stdio。
fn spawn_daemon() -> Result<(), CliError> {
    hmp_daemon::serve::spawn_detached(&["--autonomous"])
        .map_err(|e| CliError::Connect(format!("拉起后端失败: {e}")))?;
    Ok(())
}

/// 轮询平台控制端点，直到 daemon 完成协议握手。
async fn wait_for_daemon(timeout: Duration) -> Result<DaemonClient, CliError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(client) = DaemonClient::try_connect().await {
            return Ok(client);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::Connect("后端启动超时".into()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn map_control_error(error: hmp_control::ControlError) -> CliError {
    match error {
        hmp_control::ControlError::Io(error) => CliError::Io(error),
        hmp_control::ControlError::Frame(error) => CliError::Protocol(error.to_string()),
        hmp_control::ControlError::Protocol(message) => CliError::Protocol(message),
    }
}
