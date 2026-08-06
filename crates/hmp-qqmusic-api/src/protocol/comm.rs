//! comm 公共参数构造与平台版本策略（对应上游 `core/versioning.py`、`core/api_context.py`）。
//!
//! HMP 面向 Linux 桌面，首版仅移植 WEB 平台；ANDROID/DESKTOP 平台留待后续。

use serde_json::{Value, json};

use crate::credential::Credential;

/// 请求平台枚举（上游 `Platform`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    /// 安卓客户端（会话/QIMEI，暂不移植）。
    Android,
    /// Windows 桌面客户端。
    Desktop,
    /// Web 端（y.qq.com）。
    Web,
}

/// 构建 WEB 平台公共参数（上游 `VersionPolicy.build_comm` WEB 分支）。
///
/// 字段顺序与上游 pydantic `CommonParams` 定义序一致（配合 `preserve_order`
/// 实现与 Python 参考实现逐字节一致的请求体）。
/// `uin` 仅在登录后存在（来自 music_id，按 int 序列化）；`g_tk` 恒存在。
pub fn build_web_comm(credential: &Credential) -> Value {
    let gt = g_tk(credential);
    let uin = parse_music_id(&credential.music_id);
    let mut map = serde_json::Map::new();
    map.insert("ct".into(), json!(24));
    map.insert("cv".into(), json!(4_747_474));
    map.insert("platform".into(), json!("yqq.json"));
    map.insert("chid".into(), json!("0"));
    if let Some(uin) = uin {
        map.insert("uin".into(), json!(uin));
    }
    map.insert("g_tk".into(), json!(gt));
    map.insert("g_tk_new_20200303".into(), json!(gt));
    map.insert("format".into(), json!("json"));
    map.insert("inCharset".into(), json!("utf-8"));
    map.insert("outCharset".into(), json!("utf-8"));
    map.insert("notice".into(), json!(0));
    map.insert("need_new_code".into(), json!(1));
    Value::Object(map)
}

/// 计算 g_tk（上游 `VersionPolicy.get_g_tk`）。
///
/// 有 music_key 时 `hash33(musickey, 5381)`，否则 5381。
pub fn g_tk(credential: &Credential) -> i64 {
    if credential.music_key.is_empty() {
        5381
    } else {
        crate::protocol::sign::hash33(&credential.music_key, 5381) as i64
    }
}

/// 由 music_id 解析 uin（上游 `credential.musicid` 为 int）。
fn parse_music_id(music_id: &str) -> Option<i64> {
    if music_id.is_empty() {
        return None;
    }
    music_id.parse().ok()
}

/// 凭证 Cookie 注入（上游 `ApiContext.prepare_http_kwargs`）。
///
/// 返回 cookie 键值对序列（顺序与上游一致）：
/// `uin`/`qqmusic_uin`（登录后）、`qm_keyst`/`qqmusic_key`（有 music key 时）。
pub fn credential_cookies(credential: &Credential) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !credential.music_id.is_empty() {
        out.push(("uin".into(), credential.music_id.clone()));
        out.push(("qqmusic_uin".into(), credential.music_id.clone()));
    }
    if !credential.music_key.is_empty() {
        out.push(("qm_keyst".into(), credential.music_key.clone()));
        out.push(("qqmusic_key".into(), credential.music_key.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(music_id: &str, music_key: &str) -> Credential {
        Credential {
            uin: music_id.into(),
            music_id: music_id.into(),
            music_key: music_key.into(),
            refresh_key: None,
            login_type: crate::credential::LoginType::Qq,
            raw_cookie: String::new(),
            ..Default::default()
        }
    }

    // Oracle 值由 Python 参考实现计算，记录于 docs/QQMUSIC_PORTING.md。

    #[test]
    fn g_tk_without_key_is_5381() {
        assert_eq!(g_tk(&credential("", "")), 5381);
    }

    #[test]
    fn g_tk_with_musickey_matches_oracle() {
        // hash33("test_music_key_123", 5381) == 988047106
        assert_eq!(g_tk(&credential("123", "test_music_key_123")), 988_047_106);
        assert_eq!(g_tk(&credential("123", "mkey123")), 1_263_162_033);
    }

    #[test]
    fn web_comm_without_credential_matches_oracle() {
        // Python: build_comm(WEB, Credential()) → 与下列 JSON 完全一致（含字段顺序）
        let comm = build_web_comm(&credential("", ""));
        assert_eq!(
            serde_json::to_string(&comm).unwrap(),
            r#"{"ct":24,"cv":4747474,"platform":"yqq.json","chid":"0","g_tk":5381,"g_tk_new_20200303":5381,"format":"json","inCharset":"utf-8","outCharset":"utf-8","notice":0,"need_new_code":1}"#
        );
    }

    #[test]
    fn web_comm_with_credential_includes_uin_and_g_tk() {
        let comm = build_web_comm(&credential("12345", "test_music_key_123"));
        assert_eq!(
            serde_json::to_string(&comm).unwrap(),
            r#"{"ct":24,"cv":4747474,"platform":"yqq.json","chid":"0","uin":12345,"g_tk":988047106,"g_tk_new_20200303":988047106,"format":"json","inCharset":"utf-8","outCharset":"utf-8","notice":0,"need_new_code":1}"#
        );
    }

    #[test]
    fn web_comm_uin_omitted_when_music_id_not_numeric() {
        let comm = build_web_comm(&credential("not-a-number", "k"));
        let obj = comm.as_object().unwrap();
        assert!(!obj.contains_key("uin"));
        assert!(obj.contains_key("g_tk"));
    }

    #[test]
    fn credential_cookies_empty_when_logged_out() {
        assert!(credential_cookies(&credential("", "")).is_empty());
    }

    #[test]
    fn credential_cookies_match_upstream_order() {
        let cookies = credential_cookies(&credential("123", "mkey"));
        assert_eq!(
            cookies,
            vec![
                ("uin".to_string(), "123".to_string()),
                ("qqmusic_uin".to_string(), "123".to_string()),
                ("qm_keyst".to_string(), "mkey".to_string()),
                ("qqmusic_key".to_string(), "mkey".to_string()),
            ]
        );
    }
}
