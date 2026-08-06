//! 凭证持久化（JSON 文件，XDG 路径）。
//!
//! 说明：PROJECT.md 规划 keyring 凭据访问（`hmp-storage`）；
//! CLI 骨架阶段先落地 JSON 文件存储，权限设为 0600，
//! 待 hmp-storage 就绪后替换。

use std::path::PathBuf;

use hmp_qqmusic_api::Credential;

/// 凭证文件路径：`$XDG_CONFIG_HOME/hmp/credential.json`。
pub fn credential_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
            PathBuf::from(home).join(".config")
        });
    base.join("hmp").join("credential.json")
}

/// 保存凭证（0600 权限）。
pub fn save(credential: &Credential) -> Result<(), Box<dyn std::error::Error>> {
    let path = credential_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(credential)?;
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        f.write_all(json.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, json)?;
    }
    Ok(())
}

/// 加载凭证；不存在时返回 `Ok(None)`。
pub fn load() -> Result<Option<Credential>, Box<dyn std::error::Error>> {
    let path = credential_path();
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&json)?))
}
