//! HMP 存储层（docs/PROJECT.md §5.2 `hmp-storage` 的凭据部分）。
//!
//! 敏感信息（QQ 音乐登录凭证）优先存入系统密钥环：
//!
//! - **Secret Service**（Linux/Arch 桌面默认，经 `gnome-keyring`/`kwallet`，
//!   使用 `keyring` crate 的 v1 兼容 API）——生产路径；
//! - **文件回退**（`HMP_CREDENTIAL_BACKEND=file` 显式启用，0600 权限，
//!   **不安全，仅供无密钥环环境**）——测试/CI 路径。
//!
//! 密钥环不可用时**不静默降级**为明文：默认后端失败直接报错，
//! 提示安装 `gnome-keyring` 或 `kwallet`。

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
pub use path::{canonical_display_path, strip_verbatim};
pub use xdg::{cache_dir, config_dir, data_dir};

/// 串行化修改进程环境变量的测试（XDG/HOME/HMP_CREDENTIAL_BACKEND）。
///
/// 这些测试直接改动全局 env，并行运行时会互相干扰（预先存在的竞态）。
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub mod path {
    //! 本地路径规范化（库键/展示路径统一拼写，勿在调用方重复实现）。

    use std::path::PathBuf;

    /// canonicalize + 剥离 Windows verbatim 前缀：库键与展示路径的统一入口。
    /// canonicalize 失败（文件暂不存在）时原样返回（与 canonical_local_key
    /// 的"保留原样待收敛"语义一致）。
    pub fn canonical_display_path(p: &std::path::Path) -> PathBuf {
        match std::fs::canonicalize(p) {
            Ok(c) => strip_verbatim(&c),
            Err(_) => p.to_path_buf(),
        }
    }

    /// 剥离已 canonicalize 路径的 Windows verbatim 前缀：`\\?\C:\a` → `C:\a`，
    /// `\\?\UNC\server\share` → `\\server\share`。其他平台无此形态，原样返回。
    /// 不剥离会让 scan_roots/local_files/tracks 的路径键带 `\\?\`，与事件路径、
    /// 用户拼写、UI 展示全部失配（前缀匹配/查询/去重失灵）。
    pub fn strip_verbatim(p: &std::path::Path) -> PathBuf {
        let text = p.as_os_str().to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest.to_owned());
        }
        p.to_path_buf()
    }
}
