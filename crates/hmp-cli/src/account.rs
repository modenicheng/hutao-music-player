//! `hmp account`：QQ 账号信息（CLI 本地凭证直连 QQ，读操作）。
//!
//! ```text
//! hmp account profile   # 主页头部（昵称等）
//! hmp account vip       # VIP 信息
//! hmp logout            # 退出登录（远端登出尽力而为 + 删本地凭证）
//! ```

use hmp_qqmusic_api::credential::Credential;
use hmp_qqmusic_api::{LoginApi, QqMusicClient, UserApi};

/// 读取本地凭证（未登录 → 错误）。
fn load_credential() -> Result<Credential, Box<dyn std::error::Error>> {
    let stored = hmp_storage::credential::store_from_env()
        .load()
        .map_err(|e| format!("failed to read credentials: {e}"))?;
    stored
        .filter(|c| c.is_logged_in())
        .ok_or_else(|| "not logged in; run `hmp login` first".into())
}

/// `hmp logout`：远端登出尽力而为（失败仅告警），本地凭证必删。
/// 未登录提示后正常退出（幂等）。
pub async fn logout() -> Result<(), Box<dyn std::error::Error>> {
    let store = hmp_storage::credential::store_from_env();
    let stored = store
        .load()
        .map_err(|e| format!("failed to read credentials: {e}"))?;
    let Some(credential) = stored.filter(|c| c.is_logged_in()) else {
        println!("Not logged in.");
        return Ok(());
    };
    if let Err(e) = LoginApi::new(&QqMusicClient::new())
        .logout(&credential)
        .await
    {
        // 远端登出失败不影响本地删除（凭证已失效亦可接受）。
        eprintln!("warning: remote logout failed ({e}); removing local credentials anyway");
    }
    store
        .delete()
        .map_err(|e| format!("failed to delete credentials: {e}"))?;
    println!(
        "Logged out (QQ {}). Desktop/daemon pick this up on their next account query.",
        credential.uin
    );
    Ok(())
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
///
/// 数据源优先级：音乐基因 `GetProfileReport`（含昵称/头像/签名）→
/// 主页 `GetHomepageHeader` 兜底（2026-09-29 起服务端对合法参数也返回
/// 10000 空壳，见 `user::UserApi::get_homepage` 文档）。
pub async fn profile(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let cred = load_credential()?;
    if cred.encrypt_uin.is_empty() {
        return Err("credential is missing encrypt_uin".into());
    }
    let client = QqMusicClient::new();
    let api = UserApi::new(&client);

    if let Ok(gene) = api.get_music_gene(&cred.encrypt_uin, Some(&cred)).await {
        if json {
            return super::output::print(&serde_json::json!({
                "uin": cred.uin,
                "nickname": gene.userinfo_card.nick_name,
                "avatar": gene.userinfo_card.head_url,
                "signature": gene.userinfo_card.signature,
                "source": "music_gene",
            }));
        }
        println!("QQ: {}", cred.uin);
        println!(
            "Nickname: {}",
            if gene.userinfo_card.nick_name.is_empty() {
                "(unknown)"
            } else {
                gene.userinfo_card.nick_name.as_str()
            }
        );
        if !gene.userinfo_card.head_url.is_empty() {
            println!("Avatar: {}", gene.userinfo_card.head_url);
        }
        if !gene.userinfo_card.signature.is_empty() {
            println!("Signature: {}", gene.userinfo_card.signature);
        }
        return Ok(());
    }

    let data = api.get_homepage(&cred.encrypt_uin, Some(&cred)).await?;
    let nick = find_str(&data, "nick")
        .or_else(|| find_str(&data, "nickname"))
        .or_else(|| find_str(&data, "name"))
        .unwrap_or("(unknown)");
    if json {
        return super::output::print(&serde_json::json!({
            "uin": cred.uin,
            "nickname": nick,
            "source": "homepage",
            "raw": data,
        }));
    }
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
pub async fn vip(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let cred = load_credential()?;
    let client = QqMusicClient::new();
    let api = UserApi::new(&client);
    let data = api.get_vip_info(&cred).await?;
    if json {
        return super::output::print(&serde_json::json!({
            "uin": cred.uin,
            "vip": data,
        }));
    }
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
