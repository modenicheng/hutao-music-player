//! HMP 媒体准备：QMC2 加密音频流下载/解密/缓存。
//!
//! 依赖 [`hmp_qqmusic_api::algorithms::qmc2`] 进行 QMC2 流密码解密。

pub mod cache;
pub mod decrypt;
pub mod stream;

#[cfg(test)]
pub(crate) mod testutil;

pub use stream::{PreparedMedia, cdn_client, prepare_media};

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

/// 播放缓存命中查找（不下载）：按稳定键（URL path | ekey，跨
/// purl 重签稳定）找已解密缓存文件，命中返回 `file://` URI（零 CDN 播放）。
pub fn cached_playable_uri(url: &str, ekey: Option<&str>) -> Result<Option<String>, MediaError> {
    let root = default_cache_root()?;
    decrypt::cached_uri_at(&root, url, ekey)
}

/// 进程内在途回填键（防同曲快速换进换出触发并发重复缓存写入），
/// 供 [`stream`] 的 tee 边播边缓存与后台补齐去重共用。
pub(crate) fn inflight_insert(key: String) -> bool {
    let mut guard = INFLIGHT_FILL.lock().unwrap();
    if guard.iter().any(|k| k == &key) {
        return false;
    }
    guard.push(key);
    true
}

pub(crate) fn inflight_remove(key: &str) {
    INFLIGHT_FILL.lock().unwrap().retain(|k| k != key);
}

static INFLIGHT_FILL: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn default_cache_root() -> Result<std::path::PathBuf, MediaError> {
    let root = hmp_storage::cache_dir().join("decrypted");
    std::fs::create_dir_all(&root)
        .map_err(|e| MediaError::Cache(format!("无法创建缓存目录: {e}")))?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// in-flight 去重：同键二次插入被拒、移除后可再插入
    /// （tee 武装与后台补齐共用的并发防护语义锚点）。
    #[test]
    fn inflight_dedup_is_key_scoped() {
        assert!(inflight_insert("k1".into()));
        assert!(!inflight_insert("k1".into()), "同键在途 → 拒绝");
        assert!(inflight_insert("k2".into()), "不同键互不影响");
        inflight_remove("k1");
        assert!(inflight_insert("k1".into()), "移除后可再插入");
        inflight_remove("k1");
        inflight_remove("k2");
    }
}
