//! 核心错误分类（docs/PROJECT.md §12）。
//!
//! UI 需将错误转换为用户可读提示，但日志保留结构化上下文；
//! 至少区分：未登录、凭证过期、接口错误、网络、音轨不可用、音质不可用等。

use thiserror::Error;

/// HMP 核心错误。
#[derive(Debug, Error)]
pub enum HmpError {
    /// 网络错误。
    #[error("network error: {0}")]
    Network(String),

    /// 需要登录。
    #[error("authentication required")]
    AuthenticationRequired,

    /// 凭证已过期。
    #[error("credential expired")]
    CredentialExpired,

    /// QQ 音乐接口错误。
    #[error("QQ Music API error {code}: {message}")]
    QqApi { code: i64, message: String },

    /// 曲目不可用（无权限/已下架等）。
    #[error("track is unavailable")]
    TrackUnavailable,

    /// 目标音质不可用（含回退链耗尽）。
    #[error("quality is unavailable")]
    QualityUnavailable,

    /// 播放错误。
    #[error("playback error: {0}")]
    Playback(String),

    /// 存储错误。
    #[error("storage error: {0}")]
    Storage(String),

    /// 响应解析失败。
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}

impl HmpError {
    /// 是否为用户可自行恢复的错误（提示后重试/重新登录）。
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            HmpError::Network(_) | HmpError::AuthenticationRequired | HmpError::CredentialExpired
        )
    }
}

/// 便捷构造：从任意 `std::io::Error` 派生网络错误。
impl From<std::io::Error> for HmpError {
    fn from(e: std::io::Error) -> Self {
        HmpError::Network(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_actionable() {
        assert_eq!(
            HmpError::AuthenticationRequired.to_string(),
            "authentication required"
        );
        assert_eq!(
            HmpError::QqApi {
                code: 10006,
                message: "denied".into()
            }
            .to_string(),
            "QQ Music API error 10006: denied"
        );
    }

    #[test]
    fn recoverability_classification() {
        assert!(HmpError::Network("timeout".into()).is_recoverable());
        assert!(HmpError::AuthenticationRequired.is_recoverable());
        assert!(HmpError::CredentialExpired.is_recoverable());
        assert!(!HmpError::TrackUnavailable.is_recoverable());
        assert!(!HmpError::QualityUnavailable.is_recoverable());
        assert!(!HmpError::Playback("boom".into()).is_recoverable());
        assert!(!HmpError::Storage("disk full".into()).is_recoverable());
    }

    #[test]
    fn io_error_converts_to_network() {
        let e = std::io::Error::new(std::io::ErrorKind::TimedOut, "dns timeout");
        let h: HmpError = e.into();
        assert!(matches!(h, HmpError::Network(_)));
        assert!(h.is_recoverable());
    }
}
