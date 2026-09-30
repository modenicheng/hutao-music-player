//! CLI → daemon 客户端（spec §4.3 `client.rs`）。

use std::path::Path;
use std::time::Duration;

use hmp_core::ipc::{Request, Response, decode_frame, encode_frame};
use hmp_daemon::transport::IpcStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    stream: IpcStream,
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

    /// 仅连接已在运行的 daemon，绝不自动拉起。`hmp quit` 专用：quit 走
    /// `connect_or_spawn` 会在 daemon 未运行时先 spawn 再退出——若 spawn
    /// 后 3s 就绪窗口内连接不上（慢盘/杀软扫描），报 startup timed out
    /// 且留下刚拉起的 daemon 继续运行。
    pub async fn connect_existing() -> Result<Self, CliError> {
        Self::try_connect(&hmp_daemon::server::socket_path()).await
    }

    async fn try_connect(path: &Path) -> Result<Self, CliError> {
        let stream = IpcStream::connect(path).await?;
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
        decode_frame::<Response>(&frame).map_err(|e| protocol_error(&e.to_string()))
    }
}

/// 响应帧解码失败（`protocol error: …`）：错误串呈 `missing field` /
/// `unknown variant` 时附一条自愈提示——daemon 是常驻单例，升级窗口内
/// 旧 daemon 仍占端点，其旧形状响应/事件在新客户端全部解不出来
/// （`hmp quit` 不受影响，Response::Ok 无需新字段，退出后重试即自愈）。
fn protocol_error(message: &str) -> CliError {
    if message.contains("missing field") || message.contains("unknown variant") {
        CliError::Protocol(format!(
            "{message}\nnote: daemon may be an older build; run `hmp quit` then retry"
        ))
    } else {
        CliError::Protocol(message.to_string())
    }
}

/// spawn `hmp serve --background`：脱离会话 + 丢弃 stdio（final review
/// Finding 8，单一 detach 点；Unix=setsid，Windows=CREATE_NO_WINDOW）。
fn spawn_daemon() -> Result<(), CliError> {
    hmp_daemon::serve::spawn_detached(&["serve", "--background"])
        .map_err(|e| CliError::Connect(format!("failed to spawn daemon: {e}")))?;
    Ok(())
}

/// 轮询端点就绪（Windows 管道不落盘，不做 exists 前置，直接探测连接）。
async fn wait_for_socket(path: &Path, timeout: Duration) -> Result<(), CliError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if IpcStream::connect(path).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::Connect("daemon startup timed out".into()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_error_hints_on_missing_field() {
        let e = protocol_error("missing field `phase` at line 1 column 200");
        let text = e.to_string();
        assert!(text.starts_with("protocol error: missing field `phase`"));
        assert!(
            text.contains("daemon may be an older build; run `hmp quit` then retry"),
            "missing field 应附旧 daemon 提示: {text}"
        );
    }

    #[test]
    fn protocol_error_hints_on_unknown_variant() {
        let e = protocol_error("unknown variant `NewFrame`, expected one of …");
        assert!(
            e.to_string().contains("daemon may be an older build"),
            "unknown variant 应附旧 daemon 提示: {e}"
        );
    }

    #[test]
    fn protocol_error_leaves_other_messages_untouched() {
        let e = protocol_error("frame shorter than the 4-byte length prefix");
        assert_eq!(
            e.to_string(),
            "protocol error: frame shorter than the 4-byte length prefix"
        );
    }
}
