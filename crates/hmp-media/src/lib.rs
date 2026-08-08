//! HMP 媒体准备：QMC2 加密音频流下载/解密/缓存。
//!
//! 依赖 [`hmp_qqmusic_api::algorithms::qmc2`] 进行 QMC2 流密码解密。

pub mod cache;
pub mod decrypt;
pub mod proxy;

#[cfg(test)]
pub(crate) mod testutil;

pub use proxy::PreparedMedia;
pub use proxy::prepare_stream;

use thiserror::Error;

/// 媒体准备过程中的错误。
#[derive(Debug, Error)]
pub enum MediaError {
    /// 网络错误（连接失败、传输中断等）。
    #[error("网络错误: {0}")]
    Network(String),

    /// HTTP 状态码非 2xx。
    #[error("HTTP {0}")]
    HttpStatus(u16),

    /// I/O 错误（文件读写）。
    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    /// QMC2 密钥解析/派生失败。
    #[error("QMC2 密钥错误: {0}")]
    Key(#[from] hmp_qqmusic_api::algorithms::qmc2::Qmc2Error),

    /// 无法识别音频格式（魔数不匹配）。
    #[error("不支持的音频格式: {0}")]
    Unsupported(String),

    /// 缓存操作错误。
    #[error("缓存错误: {0}")]
    Cache(String),
}

/// 生产入口：下载、解密、缓存至 XDG 缓存目录。
///
/// 缓存目录为 `hmp_storage::cache_dir().join("decrypted")`。
pub async fn prepare_playable(
    url: &str,
    ekey: Option<&str>,
    progress: Option<&tokio::sync::watch::Sender<Option<f64>>>,
) -> Result<String, MediaError> {
    let root = default_cache_root()?;
    decrypt::prepare_playable_at(&root, url, ekey, progress).await
}

/// 下载加密流并尝试使用文件内嵌 ekey（STag/QTag 尾部）解密。
pub async fn prepare_playable_embedded(
    url: &str,
    progress: Option<&tokio::sync::watch::Sender<Option<f64>>>,
) -> Result<String, MediaError> {
    let root = default_cache_root()?;
    decrypt::prepare_playable_embedded_at(&root, url, progress).await
}

pub(crate) fn default_cache_root() -> Result<std::path::PathBuf, MediaError> {
    let root = hmp_storage::cache_dir().join("decrypted");
    std::fs::create_dir_all(&root)
        .map_err(|e| MediaError::Cache(format!("无法创建缓存目录: {e}")))?;
    Ok(root)
}
