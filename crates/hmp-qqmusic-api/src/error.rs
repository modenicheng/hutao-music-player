//! 错误模型（docs/PROJECT.md §12 适配 + 上游 `core/exceptions.py`）。

/// HMP / qqmusic-api 统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum QqMusicError {
    /// 网络层错误（上游 `NetworkError`）。
    #[error("network error: {0}")]
    Network(String),

    /// HTTP 状态码异常（上游 `HTTPError`）。
    #[error("HTTP error {status}: {message}")]
    Http {
        /// HTTP 状态码。
        status: u16,
        /// 状态码描述。
        message: String,
    },

    /// 需要登录但未提供有效凭证（上游 `CredentialInvalidError`）。
    #[error("authentication required")]
    AuthenticationRequired,

    /// 登录凭据过期（上游 `CredentialExpiredError`）。
    #[error("credential expired")]
    CredentialExpired,

    /// 请求被限流（上游 `RatelimitedError`）。
    #[error("request rate limited")]
    Ratelimited,

    /// 请求需要签名但未提供（上游 `SignatureRequiredError`）。
    #[error("signature required for this request")]
    SignatureRequired,

    /// QQ 业务错误码（上游 `CgiApiException` / `GlobalApiError`）。
    #[error("QQ Music API error {code}: {message}")]
    QqApi {
        /// QQ 业务错误码。
        code: i64,
        /// 错误描述。
        message: String,
    },

    /// 响应数据异常（上游 `ApiDataError`）。
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// 存储层错误（HMP 扩展）。
    #[error("storage error: {0}")]
    Storage(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_human_readable() {
        assert_eq!(
            QqMusicError::QqApi {
                code: 1000,
                message: "expired".into()
            }
            .to_string(),
            "QQ Music API error 1000: expired"
        );
        assert_eq!(
            QqMusicError::Http {
                status: 503,
                message: "unavailable".into()
            }
            .to_string(),
            "HTTP error 503: unavailable"
        );
    }
}
