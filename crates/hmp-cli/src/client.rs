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
    #[error("daemon did not respond within {0:?}; it may be wedged — try `hmp quit` and retry")]
    Timeout(Duration),
}

/// 单请求超时：写请求 + 读响应 + 解码全程的硬上限（daemon 挂死时报错
/// 退出而非永久挂起，AUDIT §16 #3）。30s 为最慢合法路径（CoverGet 整图
/// 下载等 daemon 出网请求）在上游 reqwest 自身 15s 超时之外留余量。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

    /// 发请求并收响应。整体包 30s 超时（`REQUEST_TIMEOUT`）——全部远端
    /// 命令收口于此，超时对所有调用点生效（含 `hmp quit`：daemon 挂死时
    /// 30s 报错而非挂死）。超时后连接即弃（DaemonClient 随之 drop，管道
    /// 实例归还）；daemon 侧 handler 任务最多再等一次读 EOF 收场，无泄漏。
    /// Subscribe 长连接不经此路径（推送无超时语义）。
    pub async fn request(&mut self, req: &Request) -> Result<Response, CliError> {
        self.request_with_timeout(req, REQUEST_TIMEOUT).await
    }

    /// 带可调预算的请求（测试注短超时用；生产路径走 [`Self::request`]）。
    pub(crate) async fn request_with_timeout(
        &mut self,
        req: &Request,
        timeout: Duration,
    ) -> Result<Response, CliError> {
        tokio::time::timeout(timeout, async {
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
        })
        .await
        .map_err(|_| CliError::Timeout(timeout))?
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

    /// 超时路径（AUDIT §16 #3）：daemon 连上但读走请求后沉默（挂死语义）
    /// → `request_with_timeout` 在预算附近返回 `CliError::Timeout`，绝不挂死。
    /// 端点用临时路径、经模块内私有 `try_connect` 构造——`connect_existing`/
    /// `connect_or_spawn` 会碰生产端点 `\\.\pipe\hmp`，禁用（审核可选项③）。
    #[tokio::test]
    async fn request_times_out_when_daemon_silent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli-timeout.sock");
        let mut listener = hmp_daemon::transport::IpcListener::bind(&path).unwrap();
        // 服务端任务：accept → 读走完整请求帧 → 不应答，连接挂起到底。
        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut stream = listener.accept().await.unwrap();
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await.unwrap();
            let mut payload = vec![0u8; u32::from_le_bytes(len_buf) as usize];
            stream.read_exact(&mut payload).await.unwrap();
            let () = std::future::pending().await;
        });
        let mut client = DaemonClient::try_connect(&path).await.unwrap();
        let start = std::time::Instant::now();
        let err = client
            .request_with_timeout(&Request::Status, Duration::from_millis(150))
            .await
            .unwrap_err();
        assert!(
            matches!(err, CliError::Timeout(d) if d == Duration::from_millis(150)),
            "应返回 Timeout(150ms)，实际: {err:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "超时须及时返回: {:?}",
            start.elapsed()
        );
        server.abort();
    }
}
