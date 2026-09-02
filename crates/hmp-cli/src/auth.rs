//! `hmp auth`：显示登录状况（本地凭证检查，不依赖 daemon）。

use std::io::Write;

use hmp_storage::credential::{BackendKind, Credential, store_from_env};

/// 描述登录状况（纯函数，供测试）。
pub fn format_auth(cred: Option<&Credential>, backend: BackendKind) -> String {
    let Some(cred) = cred else {
        return "Not logged in; run `hmp login` first.".to_string();
    };
    let expired = if cred.is_expired() {
        "expired"
    } else {
        "valid"
    };
    let backend_name = match backend {
        BackendKind::SecretService => "system keyring (SecretService)".to_string(),
        BackendKind::File => {
            let path = hmp_storage::xdg::config_dir().join("credential.json");
            format!("plaintext file {} (insecure)", path.display())
        }
    };
    format!(
        "Logged in: yes\nUser: {} (musicid: {})\nExpiry: {}\nBackend: {}",
        cred.uin, cred.music_id, expired, backend_name
    )
}

/// 显示登录状况。
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let store = store_from_env();
    let backend = BackendKind::from_env();
    let cred = store.load()?;
    let mut out = std::io::stdout().lock();
    write!(out, "{}", format_auth(cred.as_ref(), backend))?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred() -> Credential {
        serde_json::from_str(
            r#"{"uin":"123456","music_id":"987","music_key":"abc","raw_cookie":"c","login_type":"Qq"}"#,
        )
        .unwrap()
    }

    #[test]
    fn not_logged_in() {
        assert_eq!(
            format_auth(None, BackendKind::File),
            "Not logged in; run `hmp login` first."
        );
    }

    #[test]
    fn logged_in_shows_user_and_expiry() {
        let s = format_auth(Some(&cred()), BackendKind::SecretService);
        assert!(s.contains("Logged in: yes"));
        assert!(s.contains("123456"));
        assert!(s.contains("valid")); // 无时间字段 → 视为未过期
        assert!(s.contains("keyring"));
    }

    #[test]
    fn expired_detected() {
        let c: Credential = serde_json::from_str(
            r#"{"uin":"1","music_id":"2","music_key":"k","raw_cookie":"c","login_type":"Qq",
                "musickey_create_time":1000,"key_expires_in":10}"#,
        )
        .unwrap();
        let s = format_auth(Some(&c), BackendKind::File);
        assert!(s.contains("expired"));
    }
}
