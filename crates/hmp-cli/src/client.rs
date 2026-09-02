//! CLI → daemon 客户端（spec §4.3 `client.rs`）。

use std::path::PathBuf;
use std::time::Duration;

use hmp_core::ipc::{Request, Response, decode_frame, encode_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// CLI 错误。
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("cannot connect to daemon: {0}")]
    Connect(String),
    #[error("daemon error: {code:?} {message}")]
    Response {
        code: hmp_core::IpcErrorCode,
        message: String,
    },
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// 与后端的一条连接。
pub struct DaemonClient {
    stream: UnixStream,
}

impl DaemonClient {
    /// 连接或拉起后端（ENOENT → spawn `hmp serve --background`；ECONNREFUSED → 清理重试）。
    pub async fn connect_or_spawn() -> Result<Self, CliError> {
        let path = hmp_daemon::server::socket_path();
        match Self::try_connect(&path).await {
            Ok(c) => return Ok(c),
            Err(CliError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                spawn_daemon()?;
                wait_for_socket(&path, Duration::from_secs(3)).await?;
            }
            Err(CliError::Io(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(&path);
                spawn_daemon()?;
                wait_for_socket(&path, Duration::from_secs(3)).await?;
            }
            Err(e) => return Err(e),
        }
        Self::try_connect(&path).await
    }

    async fn try_connect(path: &PathBuf) -> Result<Self, CliError> {
        let stream = UnixStream::connect(path).await?;
        Ok(Self { stream })
    }

    /// 发请求并收响应。
    pub async fn request(&mut self, req: &Request) -> Result<Response, CliError> {
        let frame = encode_frame(req).map_err(|e| CliError::Protocol(e.to_string()))?;
        self.stream.write_all(&frame).await?;
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > hmp_core::ipc::MAX_FRAME - 4 {
            return Err(CliError::Protocol("invalid frame length".into()));
        }
        let mut payload = vec![0u8; len];
        self.stream.read_exact(&mut payload).await?;
        let mut frame = Vec::with_capacity(4 + len);
        frame.extend_from_slice(&len_buf);
        frame.extend_from_slice(&payload);
        decode_frame::<Response>(&frame).map_err(|e| CliError::Protocol(e.to_string()))
    }
}

/// spawn `hmp serve --background`：经 `hmp_daemon::serve::spawn_detached` 以
/// `setsid` 脱离会话 + 丢弃 stdio（final review Finding 8，单一 detach 点）。
fn spawn_daemon() -> Result<(), CliError> {
    hmp_daemon::serve::spawn_detached(&["serve", "--background"])
        .map_err(|e| CliError::Connect(format!("failed to spawn daemon: {e}")))?;
    Ok(())
}

/// 轮询 socket 就绪。
async fn wait_for_socket(path: &PathBuf, timeout: Duration) -> Result<(), CliError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if path.exists() && UnixStream::connect(path).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::Connect("daemon startup timed out".into()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
