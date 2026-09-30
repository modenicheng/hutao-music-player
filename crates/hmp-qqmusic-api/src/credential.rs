//! 登录凭证（docs/PROJECT.md §6.4，字段对齐上游 `models/request.py::Credential`）。
//!
//! # 序列化约定
//!
//! - 本 crate 自身的持久化格式为 Rust 字段名（snake_case），由调用方
//!   （hmp-storage keyring / 文件存储）序列化与反序列化；
//! - 同时兼容**服务端登录响应形状**的 JSON：`musicid`/`musickey`/`loginType`/
//!   `musickeyCreateTime`/`keyExpiresIn`/`encryptUin` 等上游别名均已注册
//!   （serde 区分大小写，CamelCase 键必须显式 alias），数值字段容忍字符串编码；
//! - `Debug` 输出脱敏，敏感字段（music key / refresh key / cookie / token）不落日志。

use serde_json::Value;

use crate::error::QqMusicError;

/// 登录类型（上游 `Credential.login_type`，int 别名 `loginType`）。
///
/// 上游取值：`1`=微信，`2`=QQ 互联（扫码），其他值（如手机客户端扫码 `6`）按原始值保留在
/// [`LoginType::Other`] 中（`as_login_type_int` 可无损还原）。
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize)]
pub enum LoginType {
    /// QQ 扫码登录（上游 int 2）。
    #[default]
    Qq,
    /// 微信扫码登录（上游 int 1）。
    Wechat,
    /// 其他来源（上游其余 int 值，字符串保留原始数值，如 `"6"`）。
    Other(String),
}

impl LoginType {
    /// 转换为上游 `loginType` int。
    ///
    /// [`LoginType::Other`] 尝试还原构造时的原始数值（如 `"6"` → 6），
    /// 无法解析时回退为 0（上游 refresh 的 default 分支语义）。
    pub fn as_login_type_int(&self) -> i64 {
        match self {
            LoginType::Qq => 2,
            LoginType::Wechat => 1,
            LoginType::Other(v) => v.parse().unwrap_or(0),
        }
    }

    /// 由上游 `loginType` int 构造。
    ///
    /// `1` → 微信，`2` → QQ，其余值 → [`LoginType::Other`]（保留原始数值字符串）。
    pub fn from_login_type_int(v: i64) -> Self {
        match v {
            1 => LoginType::Wechat,
            2 => LoginType::Qq,
            _ => LoginType::Other(v.to_string()),
        }
    }
}

impl<'de> serde::Deserialize<'de> for LoginType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct LoginTypeVisitor;

        impl<'de> serde::de::Visitor<'de> for LoginTypeVisitor {
            type Value = LoginType;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("login type int (1/2/...) or \"Qq\"/\"Wechat\"/\"Other\"")
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(LoginType::from_login_type_int(v))
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(LoginType::from_login_type_int(v as i64))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v {
                    "Qq" | "qq" => Ok(LoginType::Qq),
                    "Wechat" | "wx" => Ok(LoginType::Wechat),
                    other => Ok(LoginType::Other(other.to_owned())),
                }
            }

            /// 兼容本 crate 派生序列化形式 `{"Other": "..."}`。
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let key: String = map
                    .next_key()?
                    .ok_or_else(|| serde::de::Error::invalid_value(
                        serde::de::Unexpected::Map,
                        &"a login type variant key",
                    ))?;
                if key == "Other" {
                    let inner: String = map.next_value()?;
                    Ok(LoginType::Other(inner))
                } else {
                    Err(serde::de::Error::unknown_field(&key, &["Other"]))
                }
            }
        }

        deserializer.deserialize_any(LoginTypeVisitor)
    }
}

/// QQ 音乐登录凭证。
///
/// 安全要求（docs/PROJECT.md §6.4）：
/// - `Debug` 输出必须脱敏，不得打印字段内容；
/// - 日志只能输出是否存在某字段；
/// - 凭据存入系统 keyring，不写普通配置文件。
///
/// 字段对齐上游 `models/request.py::Credential`：除 `uin`/`music_key`/`raw_cookie`
/// 为 HMP 侧字段外，其余均与上游同名（或注册其服务端别名）。
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Credential {
    /// 用户 QQ 号（展示用，HMP 侧字段）。
    #[serde(default)]
    pub uin: String,
    /// musicid（上游 `musicid`，登录响应中为 int，此处统一为字符串）。
    #[serde(default, alias = "musicid", deserialize_with = "de_string_from_any")]
    pub music_id: String,
    /// music key（敏感，上游 `musickey`）。
    #[serde(default, alias = "musickey")]
    pub music_key: String,
    /// refresh key（敏感，上游 `refresh_key`）。
    pub refresh_key: Option<String>,
    /// 登录类型（上游 `loginType`，int 别名已注册）。
    #[serde(default, alias = "loginType")]
    pub login_type: LoginType,
    /// 原始 Cookie（敏感，HMP 侧字段）。
    #[serde(default)]
    pub raw_cookie: String,
    /// OpenID（上游 `openid`，微信/QQ 开放平台）。
    #[serde(default, deserialize_with = "de_string_from_any")]
    pub openid: String,
    /// RefreshToken（上游 `refresh_token`，敏感）。
    #[serde(default, deserialize_with = "de_string_from_any")]
    pub refresh_token: String,
    /// AccessToken（上游 `access_token`，敏感）。
    #[serde(default, deserialize_with = "de_string_from_any")]
    pub access_token: String,
    /// 到期时间戳（上游 `expired_at`，秒）。
    #[serde(default, deserialize_with = "de_i64_lenient")]
    pub expired_at: i64,
    /// UnionID（上游 `unionid`）。
    #[serde(default, deserialize_with = "de_string_from_any")]
    pub unionid: String,
    /// 字符串形式 musicid（上游 `str_musicid`）。
    #[serde(default, deserialize_with = "de_string_from_any")]
    pub str_musicid: String,
    /// musickey 创建时间戳（上游 `musickeyCreateTime`，秒）。
    #[serde(default, alias = "musickeyCreateTime", deserialize_with = "de_i64_lenient")]
    pub musickey_create_time: i64,
    /// key 有效时长（上游 `keyExpiresIn`，秒）。
    #[serde(default, alias = "keyExpiresIn", deserialize_with = "de_i64_lenient")]
    pub key_expires_in: i64,
    /// 首次登录标记（上游 `first_login`）。
    #[serde(default, alias = "firstLogin", deserialize_with = "de_i64_lenient")]
    pub first_login: i64,
    /// 绑定账号类型（上游 `bind_account_type`，别名 `bindAccountType`）。
    #[serde(default, alias = "bindAccountType", deserialize_with = "de_i64_lenient")]
    pub bind_account_type: i64,
    /// 距下次需要 refresh key 的秒数（上游 `need_refresh_key_in`，别名 `needRefreshKeyIn`）。
    #[serde(default, alias = "needRefreshKeyIn", deserialize_with = "de_i64_lenient")]
    pub need_refresh_key_in: i64,
    /// 加密 uin（上游 `encryptUin`）。
    #[serde(default, alias = "encryptUin", deserialize_with = "de_string_from_any")]
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
            .field("first_login", &self.first_login)
            .field("bind_account_type", &self.bind_account_type)
            .field("need_refresh_key_in", &self.need_refresh_key_in)
            .field("encrypt_uin", &self.encrypt_uin)
            .finish()
    }
}

impl Credential {
    /// 是否已具备完整可用的登录态（music id + music key 均非空）。
    ///
    /// 仅做本地字段检查，不发网络请求；会话有效性需配合
    /// [`crate::login::LoginApi::check_expired`] 使用。
    pub fn is_logged_in(&self) -> bool {
        !self.music_id.is_empty() && !self.music_key.is_empty()
    }

    /// 检查凭据是否过期（上游 `Credential.is_expired`）。
    ///
    /// 依据 `musickey_create_time + key_expires_in` 与当前时间比较；
    /// 当缺少时间字段（未设置）时视为未过期（与上游一致：字段缺省为 0，
    /// `now >= 0 + 0` 恒成立会造成误判，故显式跳过）。
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
    /// 处理（与上游 pydantic 解析语义对齐）：
    /// - `loginType` 缺失时由 `musickey` 前缀推断（`W_X` 开头 → 微信，否则 → QQ）；
    /// - `musicid` 支持 int/str；数值字段容忍字符串编码（pydantic 强制转换语义）；
    /// - 缺失 `str_musicid` 时 `uin` 回退为 `musicid` 字符串。
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
        // pydantic int 字段会强制转换 "123" 等字符串编码，这里保持同等宽容度
        let get_i64 = |key: &str| -> i64 {
            match obj.get(key) {
                Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
                Some(Value::String(s)) => s.parse().unwrap_or(0),
                _ => 0,
            }
        };

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
            first_login: get_i64("first_login"),
            bind_account_type: get_i64("bindAccountType"),
            need_refresh_key_in: get_i64("needRefreshKeyIn"),
            encrypt_uin: get_str("encryptUin"),
        })
    }
}

/// 字符串字段宽容反序列化：接受字符串、整数与 null（空值 → 空串）。
///
/// 上游 `Credential.musicid` 为 int，服务端登录响应以数值下发；
/// Rust 侧统一为字符串承载，直接 alias 会在 int 形状上失败，故需此转换。
fn de_string_from_any<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StringOrNumberVisitor;

    impl<'de> serde::de::Visitor<'de> for StringOrNumberVisitor {
        type Value = String;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a string, number or null")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(v.to_owned())
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(v.to_string())
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(v.to_string())
        }

        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(v.to_string())
        }

        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(String::new())
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(String::new())
        }

        fn visit_some<D>(self, d: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            de_string_from_any(d)
        }
    }

    deserializer.deserialize_any(StringOrNumberVisitor)
}

/// 整数字段宽容反序列化：接受整数与十进制字符串（pydantic 强制转换语义），null → 0。
fn de_i64_lenient<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct I64LenientVisitor;

    impl<'de> serde::de::Visitor<'de> for I64LenientVisitor {
        type Value = i64;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an integer, numeric string or null")
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(v)
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
            i64::try_from(v).map_err(|_| serde::de::Error::invalid_value(
                serde::de::Unexpected::Unsigned(v),
                &"an integer that fits in i64",
            ))
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            v.parse().map_err(|_| serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(v),
                &"a numeric string",
            ))
        }

        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(0)
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(0)
        }

        fn visit_some<D>(self, d: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            de_i64_lenient(d)
        }
    }

    deserializer.deserialize_any(I64LenientVisitor)
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
    fn other_login_type_preserves_source_int() {
        // 上游手机扫码 loginType=6：refresh 时 comm.tmeLoginType 必须还原 6 而非 0
        let t = LoginType::from_login_type_int(6);
        assert_eq!(t, LoginType::Other("6".into()));
        assert_eq!(t.as_login_type_int(), 6);
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
    fn from_login_data_parses_string_numeric_fields() {
        // pydantic int 字段接受字符串编码（"1700000000" → 1700000000）
        let data = json!({
            "musicid": "999",
            "musickey": "k",
            "musickeyCreateTime": "1700000000",
            "keyExpiresIn": "86400"
        });
        let cred = Credential::from_login_data(&data).unwrap();
        assert_eq!(cred.music_id, "999");
        assert_eq!(cred.musickey_create_time, 1_700_000_000);
        assert_eq!(cred.key_expires_in, 86_400);
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
    fn from_login_data_extracts_upstream_extra_fields() {
        let data = json!({
            "musicid": 1,
            "musickey": "k",
            "first_login": 1,
            "bindAccountType": 2,
            "needRefreshKeyIn": 86400
        });
        let cred = Credential::from_login_data(&data).unwrap();
        assert_eq!(cred.first_login, 1);
        assert_eq!(cred.bind_account_type, 2);
        assert_eq!(cred.need_refresh_key_in, 86_400);
    }

    /// 服务端登录响应形状（CamelCase 键 + int musicid）应能直接反序列化为 Credential。
    #[test]
    fn serde_accepts_server_shaped_login_data() {
        let raw = json!({
            "musicid": 939861972,
            "musickey": "qm_keyst_value",
            "str_musicid": "939861972",
            "refresh_key": "rk",
            "loginType": 2,
            "musickeyCreateTime": 1_758_000_000,
            "keyExpiresIn": 7_776_000,
            "encryptUin": "e-uin",
            "openid": "op",
            "unionid": null
        });
        let cred: Credential = serde_json::from_value(raw).unwrap();
        assert_eq!(cred.music_id, "939861972");
        assert_eq!(cred.music_key, "qm_keyst_value");
        assert_eq!(cred.login_type, LoginType::Qq);
        assert_eq!(cred.musickey_create_time, 1_758_000_000);
        assert_eq!(cred.encrypt_uin, "e-uin");
        assert_eq!(cred.openid, "op");
        assert!(cred.is_logged_in());
    }

    /// loginType 以 int 下发时（服务端形状）反序列化后应保留原始值。
    #[test]
    fn serde_login_type_accepts_int_and_string() {
        let from_int: Credential =
            serde_json::from_value(json!({"loginType": 6, "musicid": 1, "musickey": "k"})).unwrap();
        assert_eq!(from_int.login_type, LoginType::Other("6".into()));
        assert_eq!(from_int.login_type.as_login_type_int(), 6);

        // 本 crate 持久化形状（Rust 字段名 + "Qq" 字符串）须保持兼容
        let from_rust_shape: Credential = serde_json::from_value(json!({
            "uin": "123",
            "music_id": "123",
            "music_key": "k",
            "login_type": "Qq"
        }))
        .unwrap();
        assert_eq!(from_rust_shape.login_type, LoginType::Qq);
        assert!(from_rust_shape.is_logged_in());
    }

    /// 自身序列化 → 反序列化往返无损（keyring 存储路径）。
    #[test]
    fn serde_roundtrip_preserves_all_fields() {
        let cred = Credential {
            uin: "939861972".into(),
            music_id: "939861972".into(),
            music_key: "key".into(),
            refresh_key: Some("rk".into()),
            login_type: LoginType::Other("6".into()),
            raw_cookie: "a=b".into(),
            openid: "op".into(),
            refresh_token: "rt".into(),
            access_token: "at".into(),
            expired_at: 100,
            unionid: "un".into(),
            str_musicid: "939861972".into(),
            musickey_create_time: 1,
            key_expires_in: 2,
            first_login: 3,
            bind_account_type: 4,
            need_refresh_key_in: 5,
            encrypt_uin: "e".into(),
        };
        let text = serde_json::to_string(&cred).unwrap();
        let back: Credential = serde_json::from_str(&text).unwrap();
        assert_eq!(back.uin, "939861972");
        assert_eq!(back.music_key, "key");
        assert_eq!(back.refresh_key.as_deref(), Some("rk"));
        assert_eq!(back.login_type, LoginType::Other("6".into()));
        assert_eq!(back.login_type.as_login_type_int(), 6);
        assert_eq!(back.first_login, 3);
        assert_eq!(back.bind_account_type, 4);
        assert_eq!(back.need_refresh_key_in, 5);
        assert_eq!(back.encrypt_uin, "e");
        // 敏感字段必须以明文持久化（存储层自行加密），序列化不得脱敏
        assert!(text.contains("\"music_key\":\"key\""));
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
