//! 凭据存储（keyring 优先，文件显式回退）。

use hmp_core::HmpError;
use hmp_qqmusic_api::Credential;

/// 后端类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    /// Linux Secret Service（gnome-keyring / kwallet）。
    SecretService,
    /// 明文 JSON 文件（0600；仅测试/无密钥环环境，显式启用）。
    File,
}

impl BackendKind {
    /// 根据环境变量选择后端：`HMP_CREDENTIAL_BACKEND=file` 强制文件回退；
    /// 否则使用 Secret Service。
    pub fn from_env() -> Self {
        match std::env::var("HMP_CREDENTIAL_BACKEND").as_deref() {
            Ok("file") => BackendKind::File,
            _ => BackendKind::SecretService,
        }
    }
}

/// 凭据存储抽象。
pub trait CredentialStore: Send + Sync {
    /// 保存凭证。
    fn save(&self, credential: &Credential) -> Result<(), HmpError>;
    /// 加载凭证（不存在返回 `Ok(None)`）。
    fn load(&self) -> Result<Option<Credential>, HmpError>;
    /// 删除凭证。
    fn delete(&self) -> Result<(), HmpError>;
}

/// Secret Service 后端（keyring v1，Linux 默认）。
///
/// 依赖 `gnome-keyring` / `kwallet` 提供 `org.freedesktop.secrets` 服务；
/// 不可用时返回 [`HmpError::Storage`] 并附带安装提示（不静默降级明文）。
#[derive(Clone, Debug)]
pub struct SecretServiceStore {
    service: &'static str,
    user: &'static str,
}

impl Default for SecretServiceStore {
    fn default() -> Self {
        Self {
            service: "hmp",
            user: "qq-music",
        }
    }
}

impl SecretServiceStore {
    /// 构造 Secret Service 凭据存储。
    pub fn new() -> Self {
        Self::default()
    }

    /// 检查密钥环可用性（`Entry::store_status`）。
    pub fn status() -> Result<(), HmpError> {
        keyring::Entry::store_status()
            .as_ref()
            .map_err(secret_service_error)?;
        Ok(())
    }
}

impl CredentialStore for SecretServiceStore {
    fn save(&self, credential: &Credential) -> Result<(), HmpError> {
        let entry =
            keyring::Entry::new(self.service, self.user).map_err(|e| secret_service_error(&e))?;
        let json = serde_json::to_vec(credential)
            .map_err(|e| HmpError::Storage(format!("serialize credential: {e}")))?;
        entry
            .set_secret(&json)
            .map_err(|e| secret_service_error(&e))
    }

    fn load(&self) -> Result<Option<Credential>, HmpError> {
        let entry =
            keyring::Entry::new(self.service, self.user).map_err(|e| secret_service_error(&e))?;
        match entry.get_secret() {
            Ok(bytes) => {
                let cred: Credential = serde_json::from_slice(&bytes)
                    .map_err(|e| HmpError::Storage(format!("deserialize credential: {e}")))?;
                Ok(Some(cred))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(secret_service_error(&e)),
        }
    }

    fn delete(&self) -> Result<(), HmpError> {
        let entry =
            keyring::Entry::new(self.service, self.user).map_err(|e| secret_service_error(&e))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(secret_service_error(&e)),
        }
    }
}

/// 文件后端（0600 权限；明文，仅测试/无密钥环环境）。
///
/// 通过 [`BackendKind::from_env`] 的 `HMP_CREDENTIAL_BACKEND=file` 显式启用。
#[derive(Clone, Debug)]
pub struct FileStore {
    path: std::path::PathBuf,
}

impl FileStore {
    /// 构造文件后端（默认 XDG 配置目录）。
    pub fn new() -> Self {
        Self::at(crate::xdg::config_dir().join("credential.json"))
    }

    /// 指定路径构造（测试用）。
    pub fn at(path: std::path::PathBuf) -> Self {
        Self { path }
    }
}

impl Default for FileStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialStore for FileStore {
    fn save(&self, credential: &Credential) -> Result<(), HmpError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| HmpError::Storage(format!("create dir: {e}")))?;
        }
        let json = serde_json::to_string_pretty(credential)
            .map_err(|e| HmpError::Storage(format!("serialize credential: {e}")))?;
        write_private(&self.path, json.as_bytes())
            .map_err(|e| HmpError::Storage(format!("write credential file: {e}")))?;
        Ok(())
    }

    fn load(&self) -> Result<Option<Credential>, HmpError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&self.path)
            .map_err(|e| HmpError::Storage(format!("read credential file: {e}")))?;
        let cred = serde_json::from_slice(&bytes)
            .map_err(|e| HmpError::Storage(format!("deserialize credential: {e}")))?;
        Ok(Some(cred))
    }

    fn delete(&self) -> Result<(), HmpError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(HmpError::Storage(format!("remove credential file: {e}"))),
        }
    }
}

/// 写文件并设置 0600 权限（unix）。
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

/// Secret Service 错误 → 可操作的 `HmpError::Storage` 提示。
fn secret_service_error(e: &keyring::Error) -> HmpError {
    HmpError::Storage(format!(
        "系统密钥环不可用（{e}）。请安装并启动 gnome-keyring 或 kwallet，\
         或使用 HMP_CREDENTIAL_BACKEND=file 回退到明文文件（不安全）"
    ))
}

/// 根据环境变量选择后端。
pub fn store_from_env() -> Box<dyn CredentialStore> {
    match BackendKind::from_env() {
        BackendKind::SecretService => Box::new(SecretServiceStore::new()),
        BackendKind::File => Box::new(FileStore::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_credential() -> Credential {
        Credential {
            uin: "10001".into(),
            music_id: "10001".into(),
            music_key: "secret-key".into(),
            refresh_key: Some("refresh-key".into()),
            login_type: Default::default(),
            raw_cookie: "skey=abc;uin=10001".into(),
            openid: String::new(),
            refresh_token: String::new(),
            access_token: String::new(),
            str_musicid: "10001".into(),
            ..Default::default()
        }
    }

    #[test]
    fn file_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::at(dir.path().join("cred.json"));
        let cred = sample_credential();

        assert!(store.load().unwrap().is_none());
        store.save(&cred).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.music_id, cred.music_id);
        assert_eq!(loaded.music_key, cred.music_key);
        assert_eq!(loaded.raw_cookie, cred.raw_cookie);
        store.delete().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn file_store_writes_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::at(dir.path().join("cred.json"));
        store.save(&sample_credential()).unwrap();
        let mode = std::fs::metadata(dir.path().join("cred.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "credential file must be 0600, got {mode:o}");
    }

    #[test]
    fn backend_kind_from_env_defaults_to_secret_service() {
        let guard = EnvGuard;
        unsafe {
            std::env::remove_var("HMP_CREDENTIAL_BACKEND");
        }
        assert_eq!(BackendKind::from_env(), BackendKind::SecretService);
        guard.restore();
    }

    #[test]
    fn backend_kind_from_env_file_override() {
        let guard = EnvGuard;
        unsafe {
            std::env::set_var("HMP_CREDENTIAL_BACKEND", "file");
        }
        assert_eq!(BackendKind::from_env(), BackendKind::File);
        guard.restore();
    }

    #[test]
    fn secret_service_status_reports_clearly_when_unavailable() {
        // 无 Secret Service 环境：应为明确的 Storage 错误；有则 Ok。
        // 此测试在两个分支下都应通过（不 panic），验证错误映射而非后端本身。
        match SecretServiceStore::status() {
            Ok(()) => {}
            Err(HmpError::Storage(msg)) => {
                assert!(msg.contains("密钥环"), "error should guide user: {msg}");
            }
            Err(other) => panic!("unexpected error type: {other:?}"),
        }
    }

    struct EnvGuard;
    impl EnvGuard {
        fn restore(&self) {
            unsafe {
                std::env::remove_var("HMP_CREDENTIAL_BACKEND");
            }
        }
    }
}
