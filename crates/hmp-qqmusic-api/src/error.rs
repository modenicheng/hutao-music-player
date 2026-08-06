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

    /// 登录域业务错误（上游 `LoginError`）。
    #[error("login error {code}: {message}")]
    Login {
        /// 登录业务错误码。
        code: i64,
        /// 错误描述。
        message: String,
    },

    /// 登录鉴权参数无效或已过期（上游 `LoginAuthExpiredError`，code 1000/104401/104400）。
    #[error("login auth expired or invalid")]
    LoginAuthExpired,

    /// 登录设备数量超限（上游 `LoginDeviceLimitError`，code 20279）。
    #[error("login device limit reached")]
    LoginDeviceLimit,

    /// 账号受限或已被封禁（上游 `LoginAccountRestrictedError`，code 20277/20278/20450）。
    #[error("login account restricted or banned")]
    LoginAccountRestricted,

    /// 登录操作过于频繁（上游 `LoginRateLimitError`，code 104604）。
    #[error("login rate limited")]
    LoginRateLimit,

    /// 凭证刷新失败（上游 `CredentialRefreshError`，包装登录错误）。
    #[error("credential refresh failed: {code}: {message}")]
    CredentialRefresh {
        /// 原始登录错误码。
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
        assert_eq!(
            QqMusicError::LoginAuthExpired.to_string(),
            "login auth expired or invalid"
        );
        assert_eq!(
            QqMusicError::Login {
                code: 20261,
                message: "登录参数错误".into()
            }
            .to_string(),
            "login error 20261: 登录参数错误"
        );
        assert_eq!(
            QqMusicError::CredentialRefresh {
                code: 20271,
                message: "验证码错误".into()
            }
            .to_string(),
            "credential refresh failed: 20271: 验证码错误"
        );
    }
}
