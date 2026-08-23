//! HMP 存储层（docs/PROJECT.md §5.2 `hmp-storage` 的凭据部分）。
//!
//! 敏感信息（QQ 音乐登录凭证）优先存入系统密钥环：
//!
//! - Windows 使用 **Credential Manager**；Linux 使用 **Secret Service**
//!   （`gnome-keyring`/`kwallet`）；
//! - **文件回退**（`HMP_CREDENTIAL_BACKEND=file` 显式启用，0600 权限，
//!   **不安全，仅供无密钥环环境**）——测试/CI 路径。
//!
//! 密钥环不可用时**不静默降级**为明文：默认后端失败并给出平台提示。

pub mod config;
pub mod credential;
pub mod db;
pub mod local;
pub mod scan;
pub mod xdg;

pub use config::{Config, QualityMode, QualityPref};
pub use credential::{BackendKind, CredentialStore, FileStore, SecretServiceStore};
pub use db::{
    FavoriteRow, LibraryDb, PlayEnd, PlaylistOpRow, PlaylistRow, PlaylistTrackRow, RecentPlay,
    RelationRow, ScanOutcome, TrackMeta, TrackRow,
};
pub use local::{LocalMeta, is_audio_ext, read_meta};
pub use xdg::{cache_dir, config_dir, data_dir};

/// 串行化修改进程环境变量的测试（应用目录/HMP_CREDENTIAL_BACKEND）。
///
/// 这些测试直接改动全局 env，并行运行时会互相干扰（预先存在的竞态）。
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
