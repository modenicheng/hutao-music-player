//! 登录凭证（docs/PROJECT.md §6.4）。

use serde_json::Value;

use crate::error::QqMusicError;

/// 登录类型。
///
/// 上游 `loginType` 为 int：`1`=微信，`2`=QQ，其他=0。
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum LoginType {
    /// QQ 扫码登录（上游 int 2）。
    #[default]
    Qq,
    /// 微信扫码登录（上游 int 1）。
    Wechat,
    /// 其他来源（上游 int 0）。
    Other(String),
}

impl LoginType {
    /// 转换为上游 `loginType` int。
    pub fn as_login_type_int(&self) -> i64 {
        match self {
            LoginType::Qq => 2,
            LoginType::Wechat => 1,
            LoginType::Other(_) => 0,
        }
    }

    /// 由上游 `loginType` int 构造。
    pub fn from_login_type_int(v: i64) -> Self {
        match v {
            1 => LoginType::Wechat,
            2 => LoginType::Qq,
            _ => LoginType::Other(v.to_string()),
        }
    }
}

/// QQ 音乐登录凭证。
///
/// 安全要求（docs/PROJECT.md §6.4）：
/// - `Debug` 输出必须脱敏，不得打印字段内容；
/// - 日志只能输出是否存在某字段；
/// - 凭据存入系统 keyring，不写普通配置文件。
///
/// 字段对齐上游 `models/request.py::Credential`：
/// 除 `uin`/`music_key`/`raw_cookie` 外均为上游同名（或 alias）字段。
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Credential {
    /// 用户 QQ 号（展示用）。
    pub uin: String,
    /// musicid（上游 `musicid`）。
    pub music_id: String,
    /// music key（敏感，上游 `musickey`）。
    pub music_key: String,
    /// refresh key（敏感，上游 `refresh_key`）。
    pub refresh_key: Option<String>,
    /// 登录类型。
    pub login_type: LoginType,
    /// 原始 Cookie（敏感）。
    pub raw_cookie: String,
    /// OpenID（上游 `openid`，微信/QQ 开放平台）。
    #[serde(default)]
    pub openid: String,
    /// RefreshToken（上游 `refresh_token`，敏感）。
    #[serde(default)]
    pub refresh_token: String,
    /// AccessToken（上游 `access_token`，敏感）。
    #[serde(default)]
    pub access_token: String,
    /// 到期时间戳（上游 `expired_at`，秒）。
    #[serde(default)]
    pub expired_at: i64,
    /// UnionID（上游 `unionid`）。
    #[serde(default)]
    pub unionid: String,
    /// 字符串形式 musicid（上游 `str_musicid`）。
    #[serde(default)]
    pub str_musicid: String,
    /// musickey 创建时间戳（上游 `musickeyCreateTime`，秒）。
    #[serde(default, alias = "musickeyCreateTime")]
    pub musickey_create_time: i64,
    /// key 有效时长（上游 `keyExpiresIn`，秒）。
    #[serde(default, alias = "keyExpiresIn")]
    pub key_expires_in: i64,
    /// 加密 uin（上游 `encryptUin`）。
    #[serde(default, alias = "encryptUin")]
    pub encrypt_uin: String,
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
            .field("openid", &self.openid)
            .field("refresh_token", &"<redacted>")
            .field("access_token", &"<redacted>")
            .field("expired_at", &self.expired_at)
            .field("unionid", &self.unionid)
            .field("str_musicid", &self.str_musicid)
            .field("musickey_create_time", &self.musickey_create_time)
            .field("key_expires_in", &self.key_expires_in)
            .field("encrypt_uin", &self.encrypt_uin)
            .finish()
    }
}

impl Credential {
    /// 是否已具备完整可用的登录态（music id + music key）。
    pub fn is_logged_in(&self) -> bool {
        !self.music_id.is_empty() && !self.music_key.is_empty()
    }

    /// 检查凭据是否过期（上游 `Credential.is_expired`）。
    ///
    /// 依据 `musickey_create_time + key_expires_in` 与当前时间比较；
    /// 当缺少时间字段（未设置）时视为未过期。
    pub fn is_expired(&self) -> bool {
        if self.musickey_create_time == 0 || self.key_expires_in == 0 {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        now >= self.musickey_create_time + self.key_expires_in
    }

    /// 由登录 CGI 响应的 `data` 对象构造凭证（上游 `LoginApi._validate_result` +
    /// `Credential.model_validate`）。
    ///
    /// 处理：
    /// - `loginType` 缺失时由 `musickey` 前缀推断（`W_X` 开头 → 微信，否则 → QQ）；
    /// - `musicid` 支持 int/str；
    /// - 缺失 `str_musicid` 时回退为 `musicid` 字符串。
    ///
    /// `data` 必须为 JSON 对象，否则返回 [`QqMusicError::InvalidResponse`]。
    pub fn from_login_data(data: &Value) -> Result<Self, QqMusicError> {
        let obj = data
            .as_object()
            .ok_or_else(|| QqMusicError::InvalidResponse("login data is not an object".into()))?;

        let music_id = match obj.get("musicid") {
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        let music_key = obj
            .get("musickey")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();

        // 推断登录类型（上游 _infer_login_type）
        let login_type = if obj.contains_key("loginType") || obj.contains_key("login_type") {
            let v = obj
                .get("loginType")
                .or_else(|| obj.get("login_type"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            LoginType::from_login_type_int(v)
        } else if music_key.starts_with("W_X") {
            LoginType::Wechat
        } else {
            LoginType::Qq
        };

        let str_musicid = obj
            .get("str_musicid")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();

        let get_str = |key: &str| {
            obj.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        let get_i64 = |key: &str| obj.get(key).and_then(|v| v.as_i64()).unwrap_or(0);

        Ok(Credential {
            uin: if str_musicid.is_empty() {
                music_id.clone()
            } else {
                str_musicid.clone()
            },
            music_id,
            music_key,
            refresh_key: {
                let rk = get_str("refresh_key");
                if rk.is_empty() { None } else { Some(rk) }
            },
            login_type,
            raw_cookie: String::new(),
            openid: get_str("openid"),
            refresh_token: get_str("refresh_token"),
            access_token: get_str("access_token"),
            expired_at: get_i64("expired_at"),
            unionid: get_str("unionid"),
            str_musicid,
            musickey_create_time: get_i64("musickeyCreateTime"),
            key_expires_in: get_i64("keyExpiresIn"),
            encrypt_uin: get_str("encryptUin"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Credential {
        Credential {
            uin: "123456".into(),
            music_id: "mid".into(),
            music_key: "secret-key".into(),
            refresh_key: Some("secret-refresh".into()),
            login_type: LoginType::Qq,
            raw_cookie: "uin=123456; qm_keyst=secret-key".into(),
            ..Default::default()
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

    #[test]
    fn login_type_int_roundtrip() {
        assert_eq!(LoginType::Qq.as_login_type_int(), 2);
        assert_eq!(LoginType::Wechat.as_login_type_int(), 1);
        assert_eq!(LoginType::Other("x".into()).as_login_type_int(), 0);
        assert_eq!(LoginType::from_login_type_int(2), LoginType::Qq);
        assert_eq!(LoginType::from_login_type_int(1), LoginType::Wechat);
        assert!(matches!(
            LoginType::from_login_type_int(9),
            LoginType::Other(_)
        ));
    }

    #[test]
    fn from_login_data_parses_qq_response() {
        let data = json!({
            "musicid": 12345,
            "musickey": "mkey_abc",
            "str_musicid": "12345",
            "refresh_key": "rk_xyz",
            "loginType": 2,
            "musickeyCreateTime": 1_700_000_000,
            "keyExpiresIn": 86_400,
            "encryptUin": "e123"
        });
        let cred = Credential::from_login_data(&data).unwrap();
        assert_eq!(cred.music_id, "12345");
        assert_eq!(cred.music_key, "mkey_abc");
        assert_eq!(cred.refresh_key.as_deref(), Some("rk_xyz"));
        assert_eq!(cred.login_type, LoginType::Qq);
        assert_eq!(cred.uin, "12345");
        assert_eq!(cred.musickey_create_time, 1_700_000_000);
        assert_eq!(cred.key_expires_in, 86_400);
        assert_eq!(cred.encrypt_uin, "e123");
        assert!(cred.is_logged_in());
    }

    #[test]
    fn from_login_data_infers_login_type_from_musickey_prefix() {
        let data = json!({"musicid": 1, "musickey": "W_X_prefix_key"});
        let cred = Credential::from_login_data(&data).unwrap();
        assert_eq!(cred.login_type, LoginType::Wechat);
    }

    #[test]
    fn from_login_data_defaults_login_type_to_qq() {
        let data = json!({"musicid": 1, "musickey": "plain_key"});
        let cred = Credential::from_login_data(&data).unwrap();
        assert_eq!(cred.login_type, LoginType::Qq);
    }

    #[test]
    fn from_login_data_rejects_non_object() {
        assert!(matches!(
            Credential::from_login_data(&json!([1, 2])),
            Err(QqMusicError::InvalidResponse(_))
        ));
    }

    #[test]
    fn from_login_data_str_musicid_fallback_to_musicid() {
        let data = json!({"musicid": 777});
        let cred = Credential::from_login_data(&data).unwrap();
        assert_eq!(cred.uin, "777");
        assert_eq!(cred.str_musicid, "");
    }

    #[test]
    fn is_expired_compares_create_time_plus_ttl() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let mut cred = sample();
        // 未设置时间字段 → 未过期
        assert!(!cred.is_expired());

        // 已过期：创建于 10 天前，TTL 1 天
        cred.musickey_create_time = now - 10 * 86_400;
        cred.key_expires_in = 86_400;
        assert!(cred.is_expired());

        // 未过期：创建于 10 分钟前，TTL 1 天
        cred.musickey_create_time = now - 600;
        assert!(!cred.is_expired());
    }
}
