//! `hmp account`：QQ 账号信息（CLI 本地凭证直连 QQ，读操作）。
//!
//! ```text
//! hmp account profile   # 主页头部（昵称等）
//! hmp account vip       # VIP 信息
//! ```

use hmp_qqmusic_api::{QqMusicClient, UserApi, credential::Credential};

/// 读取本地凭证（未登录 → 错误）。
fn load_credential() -> Result<Credential, Box<dyn std::error::Error>> {
    let stored = hmp_storage::credential::store_from_env()
        .load()
        .map_err(|e| format!("failed to read credentials: {e}"))?;
    stored
        .filter(|c| c.is_logged_in())
        .ok_or_else(|| "not logged in; run `hmp login` first".into())
}

/// 展示型字段提取：从 JSON 的任意层级找第一个指定 key 的字符串值。
fn find_str<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get(key) {
                return Some(s);
            }
            map.values().find_map(|sub| find_str(sub, key))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|sub| find_str(sub, key)),
        _ => None,
    }
}

/// 主页头部（昵称/头像等；展示型）。
pub async fn profile() -> Result<(), Box<dyn std::error::Error>> {
    let cred = load_credential()?;
    if cred.encrypt_uin.is_empty() {
        return Err("credential is missing encrypt_uin".into());
    }
    let client = QqMusicClient::new();
    let api = UserApi::new(&client);
    let data = api.get_homepage(&cred.encrypt_uin, Some(&cred)).await?;
    let nick = find_str(&data, "nick")
        .or_else(|| find_str(&data, "nickname"))
        .or_else(|| find_str(&data, "name"))
        .unwrap_or("(unknown)");
    println!("QQ: {}", cred.uin);
    println!("Nickname: {nick}");
    // 原始头部摘要（保留扩展空间）。
    if let Some(obj) = data.as_object() {
        for k in ["gender", "level", "city", "signature"] {
            if let Some(v) = obj.get(k) {
                println!("{k}: {v}");
            }
        }
    }
    Ok(())
}

/// VIP 信息（展示型）。
pub async fn vip() -> Result<(), Box<dyn std::error::Error>> {
    let cred = load_credential()?;
    let client = QqMusicClient::new();
    let api = UserApi::new(&client);
    let data = api.get_vip_info(&cred).await?;
    println!("VIP info: {data}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_str_searches_nested() {
        let v = serde_json::json!({
            "data": { "userinfo": { "nick": "胡桃", "level": 3 } }
        });
        assert_eq!(find_str(&v, "nick"), Some("胡桃"));
        assert_eq!(find_str(&v, "nonexistent"), None);
        assert_eq!(
            find_str(&serde_json::json!({"a": [{"b": "x"}]}), "b"),
            Some("x")
        );
    }
}
