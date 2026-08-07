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

pub mod credential;
pub mod xdg;

pub use credential::{BackendKind, CredentialStore, FileStore, SecretServiceStore};
pub use xdg::{cache_dir, config_dir, data_dir};

/// 串行化修改进程环境变量的测试（XDG/HOME/HMP_CREDENTIAL_BACKEND）。
///
/// 这些测试直接改动全局 env，并行运行时会互相干扰（预先存在的竞态）。
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
