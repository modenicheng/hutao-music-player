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

/// 播放缓存命中查找（不下载不代理）：按稳定键（URL path | ekey，跨
/// purl 重签稳定）找已解密缓存文件，命中返回 `file://` URI（零 CDN 播放）。
pub fn cached_playable_uri(url: &str, ekey: Option<&str>) -> Result<Option<String>, MediaError> {
    let root = default_cache_root()?;
    decrypt::cached_uri_at(&root, url, ekey)
}

/// 后台回填播放缓存：全量下载（+解密）进 `cache_dir()/decrypted`，
/// 容量驱逐与命中校验同回退路径。幂等：已缓存或同键回填进行中 →
/// `Ok(None)`；本调用完成回填 → `Ok(Some(file:// URI))`。
///
/// 供 daemon 播放解析在需要时 spawn；流式播放路径的 tee 边播边缓存
/// 共享同一去重键空间（[`INFLIGHT_FILL`]），两者不会重复下载。
pub async fn cache_fill(url: &str, ekey: Option<&str>) -> Result<Option<String>, MediaError> {
    let root = default_cache_root()?;
    let key = cache::cache_key(url, ekey.unwrap_or(""));
    if !inflight_insert(key.clone()) {
        return Ok(None);
    }
    let result = decrypt::cache_fill_at(&root, url, ekey).await;
    inflight_remove(&key);
    result.map(Some)
}

/// 进程内在途回填键（防同曲快速换进换出触发并发重复全量下载）。
/// tee 边播边缓存（[`stream`]）与 [`cache_fill`] 共用。
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
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 并发回填同键：恰一个执行下载、另一个 `Ok(None)`（in-flight 去重）。
    /// XDG 隔离防污染真实缓存目录（本 crate 无其他 cache_dir 读者）。
    #[tokio::test]
    async fn cache_fill_dedupes_concurrent_same_key() {
        let _env = testutil::isolate_cache("fill_dedup");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"fLaC-payload".to_vec())
                    .set_delay(std::time::Duration::from_millis(300)),
            )
            .expect(1)
            .mount(&server)
            .await;
        let url = format!("{}/song.mp3", server.uri());

        let (r1, r2) = tokio::join!(
            cache_fill(&url, None::<&str>),
            cache_fill(&url, None::<&str>)
        );
        let exactly_one = matches!(
            (&r1, &r2),
            (Ok(Some(_)), Ok(None)) | (Ok(None), Ok(Some(_)))
        );
        assert!(exactly_one, "恰一个回填执行: r1={r1:?} r2={r2:?}");
    }
}
