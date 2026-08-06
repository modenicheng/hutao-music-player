//! 登录凭证（docs/PROJECT.md §6.4）。

/// 登录类型。
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum LoginType {
    /// QQ 扫码登录。
    #[default]
    Qq,
    /// 微信扫码登录。
    Wechat,
    /// 其他来源。
    Other(String),
}

/// QQ 音乐登录凭证。
///
/// 安全要求（docs/PROJECT.md §6.4）：
/// - `Debug` 输出必须脱敏，不得打印字段内容；
/// - 日志只能输出是否存在某字段；
/// - 凭据存入系统 keyring，不写普通配置文件。
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Credential {
    /// 用户 QQ 号。
    pub uin: String,
    /// musicid（凭据关联标识）。
    pub music_id: String,
    /// music key（敏感）。
    pub music_key: String,
    /// refresh key（敏感，可选）。
    pub refresh_key: Option<String>,
    /// 登录类型。
    pub login_type: LoginType,
    /// 原始 Cookie（敏感）。
    pub raw_cookie: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("uin", &self.uin)
            .field("music_id", &self.music_id)
            .field("music_key", &"<redacted>")
            .field(
                "refresh_key",
                &self.refresh_key.as_ref().map(|_| "<redacted>"),
            )
            .field("login_type", &self.login_type)
            .field("raw_cookie", &"<redacted>")
            .finish()
    }
}

impl Credential {
    /// 是否已具备完整可用的登录态（music id + music key）。
    pub fn is_logged_in(&self) -> bool {
        !self.music_id.is_empty() && !self.music_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Credential {
        Credential {
            uin: "123456".into(),
            music_id: "mid".into(),
            music_key: "secret-key".into(),
            refresh_key: Some("secret-refresh".into()),
            login_type: LoginType::Qq,
            raw_cookie: "uin=123456; qm_keyst=secret-key".into(),
        }
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let debug = format!("{:?}", sample());
        assert!(!debug.contains("secret-key"));
        assert!(!debug.contains("secret-refresh"));
        assert!(!debug.contains("qm_keyst"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("123456")); // uin 非敏感
    }

    #[test]
    fn logged_in_requires_both_id_and_key() {
        let mut cred = sample();
        assert!(cred.is_logged_in());
        cred.music_key.clear();
        assert!(!cred.is_logged_in());
        cred.music_key = "k".into();
        cred.music_id.clear();
        assert!(!cred.is_logged_in());
    }
}
