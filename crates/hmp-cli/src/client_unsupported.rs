//! CLI client fallback when the daemon's Unix-socket transport is unavailable.

use hmp_core::ipc::{Request, Response};

/// CLI error. The variants match the Unix client so command modules stay platform-neutral.
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

/// Placeholder with the same API as the Unix transport client.
#[derive(Debug)]
pub struct DaemonClient;

impl DaemonClient {
    pub async fn connect_or_spawn() -> Result<Self, CliError> {
        Err(CliError::Connect(
            "Windows 尚未提供 daemon IPC；请使用 HMP 原生桌面应用".to_owned(),
        ))
    }

    pub async fn request(&mut self, _req: &Request) -> Result<Response, CliError> {
        Err(CliError::Connect("当前平台不支持 daemon IPC".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::DaemonClient;

    #[tokio::test]
    async fn reports_platform_limit_without_attempting_unix_io() {
        let error = DaemonClient::connect_or_spawn().await.unwrap_err();
        assert!(error.to_string().contains("Windows"));
    }
}
