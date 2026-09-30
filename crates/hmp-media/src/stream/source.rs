//! CDN 流式数据源：探测 CDN Range 支持 → 解析 QMC2 尾部 →
//! 构建流密码 → 进程内随机访问解密源（[`hmp_core::MediaStreamSource`]）。
//!
//! 若 CDN 不支持 `Range`（返回 200 或无 `Content-Range`），
//! 自动回退到 `decrypt` 全量下载-解密-缓存流程（`file://` URI）。
//!
//! 流式路径返回 [`PreparedMedia`]：`uri` 保留原 CDN url（仅元数据/日志），
//! 播放字节流经 `source`（[`DecryptReader`：同步 `Read + Seek`，后台预取）
//! 直供播放器，取代历史上的 `127.0.0.1` 回环 HTTP 代理。

use std::io;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::{Stream, StreamExt};
use hmp_qqmusic_api::algorithms::qmc2::{self, Footer, Qmc2Cipher};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use super::reader::{DecryptReader, ReaderParams, SourceContext, TeePlan};
use crate::MediaError;
use crate::decrypt;
use crate::decrypt::embedded_ekey_from_bytes;

// ── 公共类型 ────────────────────────────────────────────────────────

/// 已就绪的媒体。
///
/// 流式路径：`uri` = 原 CDN url（元数据），`source` = 进程内随机访问源；
/// 回退路径：`uri` = `file://` 缓存文件，`source` = `None`。
pub struct PreparedMedia {
    /// 元数据 URI：流式路径 = 原 CDN url；回退路径 = `file://` 缓存文件。
    pub uri: String,
    /// 进程内随机访问源；回退（`file://`）时为 `None`。
    pub source: Option<Arc<dyn hmp_core::MediaStreamSource>>,
    /// 测试钩子：具体类型源（`dyn` 无法 downcast，测试观测内部状态用）。
    #[cfg(test)]
    pub(crate) concrete: Option<Arc<StreamSource>>,
}

impl PreparedMedia {
    /// 流式路径（进程内 source）。
    fn streaming(uri: String, source: Arc<StreamSource>) -> Self {
        #[allow(unused_mut)]
        let mut pm = Self {
            uri,
            source: Some(Arc::clone(&source) as Arc<dyn hmp_core::MediaStreamSource>),
            #[cfg(test)]
            concrete: None,
        };
        #[cfg(test)]
        {
            pm.concrete = Some(source);
        }
        pm
    }

    /// 回退路径（`file://` 缓存文件）。
    fn fallback(uri: String) -> Self {
        #[allow(unused_mut)]
        let mut pm = Self {
            uri,
            source: None,
            #[cfg(test)]
            concrete: None,
        };
        pm
    }
}

/// 闭区间 `[start, end]`（含两端）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ByteRange {
    /// 区间起始字节（含）。
    pub start: u64,
    /// 区间结束字节（含）。
    pub end: u64,
}

/// 明文（无 ekey 且无 footer）直通密码：QMC2 trait 的空实现。
struct IdentityCipher;

impl Qmc2Cipher for IdentityCipher {
    fn decrypt(&self, _offset: usize, _buf: &mut [u8]) {}
}

// ── 流式数据源 ──────────────────────────────────────────────────────

/// CDN 区间拉取 + 按需 QMC2 解密的随机访问数据源。
///
/// 实现 [`hmp_core::MediaStreamSource`]：`open` 构造 [`DecryptReader`]
/// （同步 `Read + Seek`，内部启动后台预取任务，需在 tokio 上下文调用）。
pub(crate) struct StreamSource {
    ctx: Arc<SourceContext>,
}

impl StreamSource {
    /// 以测试参数构造（小窗口/回看值驱动窗口行为断言）。
    fn with_params(
        client: reqwest::Client,
        cdn_url: &str,
        cipher: Arc<dyn Qmc2Cipher>,
        audio_len: u64,
        total_len: u64,
        tee: Option<TeePlan>,
        params: ReaderParams,
    ) -> Self {
        Self {
            ctx: Arc::new(SourceContext {
                client,
                cdn_url: cdn_url.to_owned(),
                cipher,
                audio_len,
                total_len,
                sem: Arc::new(Semaphore::new(4)),
                tee,
                params,
            }),
        }
    }

    /// 打开一个具体类型的 reader（`MediaStreamSource::open` 的内部实现；
    /// 测试直接持有具体类型以观测内部状态）。
    pub(crate) fn open_reader(&self) -> io::Result<DecryptReader> {
        DecryptReader::spawn(Arc::clone(&self.ctx))
    }
}

impl hmp_core::MediaStreamSource for StreamSource {
    fn len(&self) -> u64 {
        self.ctx.audio_len
    }

    fn open(&self) -> io::Result<Box<dyn hmp_core::MediaStream>> {
        Ok(Box::new(self.open_reader()?))
    }
}

impl std::fmt::Debug for StreamSource {
    /// 手写 Debug：勿打印完整 CDN url（带签名 query），只保留 host + path。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamSource")
            .field("cdn", &redact_url(&self.ctx.cdn_url))
            .field("audio_len", &self.ctx.audio_len)
            .field("total_len", &self.ctx.total_len)
            .finish_non_exhaustive()
    }
}

/// URL 脱敏：仅 host + path（query 含一次性签名，且可能很长）。
fn redact_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let host = u.host().map(|h| h.to_string()).unwrap_or_default();
            format!("{host}{}", u.path())
        }
        Err(_) => url.split(['?', '#']).next().unwrap_or(url).to_string(),
    }
}

pub(crate) type OwnedChunkStream = Pin<Box<dyn Stream<Item = io::Result<Vec<u8>>> + Send>>;

/// 拉取并验证一个区间。返回的流由调用方驱动（生产者任务 / tee 补齐任务）。
pub(crate) async fn fetch_and_decrypt_range(
    client: reqwest::Client,
    cdn_url: String,
    cipher: Arc<dyn Qmc2Cipher>,
    sem: Arc<Semaphore>,
    range: ByteRange,
    total_len: u64,
) -> io::Result<OwnedChunkStream> {
    let permit = sem
        .acquire_owned()
        .await
        .map_err(|e| io::Error::other(format!("failed to acquire semaphore: {e}")))?;
    let start = range.start;
    let end = range.end;
    let expected_len = end - start + 1;
    let range_header = format!("bytes={start}-{end}");
    debug!(cdn_url = %cdn_url, %range_header, "请求 CDN 区间");

    let response = client
        .get(&cdn_url)
        .header("Range", &range_header)
        .send()
        .await
        .map_err(|e| io::Error::other(format!("CDN request failed: {e}")))?;
    let status = response.status();

    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        let valid_range = response
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| parse_content_range(v) == Some((start, end, total_len)));
        if !valid_range {
            return Err(io::Error::other(
                "CDN 206 Content-Range does not match the request",
            ));
        }
        if response
            .content_length()
            .is_some_and(|len| len != expected_len)
        {
            return Err(io::Error::other(
                "CDN 206 body length does not match the requested range",
            ));
        }
        let byte_stream = response.bytes_stream();
        Ok(Box::pin(futures_util::stream::unfold(
            (byte_stream, start, 0_u64, false, permit),
            move |(mut byte_stream, offset, delivered, finished, permit)| {
                let cipher = Arc::clone(&cipher);
                async move {
                    if finished {
                        return None;
                    }
                    match byte_stream.next().await {
                        Some(Ok(chunk)) => {
                            let chunk_len = chunk.len() as u64;
                            if chunk_len > expected_len.saturating_sub(delivered) {
                                return Some((
                                    Err(io::Error::other(
                                        "CDN 206 body exceeds the requested range",
                                    )),
                                    (byte_stream, offset, delivered, true, permit),
                                ));
                            }
                            let mut output = chunk.to_vec();
                            cipher.decrypt(offset as usize, &mut output);
                            Some((
                                Ok(output),
                                (
                                    byte_stream,
                                    offset + chunk_len,
                                    delivered + chunk_len,
                                    false,
                                    permit,
                                ),
                            ))
                        }
                        Some(Err(e)) => Some((
                            Err(io::Error::other(format!("stream read error: {e}"))),
                            (byte_stream, offset, delivered, true, permit),
                        )),
                        None if delivered == expected_len => None,
                        None => Some((
                            Err(io::Error::other(
                                "CDN 206 body ended before the requested range",
                            )),
                            (byte_stream, offset, delivered, true, permit),
                        )),
                    }
                }
            },
        )))
    } else if status == reqwest::StatusCode::OK {
        warn!(cdn_url = %cdn_url, "CDN returned 200 (Range ignored); streaming skip");
        let byte_stream = response.bytes_stream();
        Ok(Box::pin(futures_util::stream::unfold(
            (byte_stream, start, start, 0_u64, false, permit),
            move |(byte_stream, skip_remaining, decrypt_offset, delivered, finished, permit)| {
                let cipher = Arc::clone(&cipher);
                async move {
                    if finished {
                        return None;
                    }
                    let mut byte_stream = byte_stream;
                    let mut skip_remaining = skip_remaining;
                    let decrypt_offset = decrypt_offset;
                    let delivered = delivered;
                    loop {
                        match byte_stream.next().await {
                            Some(Ok(chunk)) => {
                                if skip_remaining > 0 {
                                    let chunk_len = chunk.len() as u64;
                                    if chunk_len <= skip_remaining {
                                        skip_remaining -= chunk_len;
                                        continue;
                                    }
                                    // chunk straddles the start boundary
                                    let start_idx = skip_remaining as usize;
                                    let remaining = expected_len.saturating_sub(delivered) as usize;
                                    let take = remaining.min(chunk.len() - start_idx);
                                    let mut output = chunk[start_idx..start_idx + take].to_vec();
                                    cipher.decrypt(decrypt_offset as usize, &mut output);
                                    let new_delivered = delivered + take as u64;
                                    let done = new_delivered == expected_len;
                                    return Some((
                                        Ok(output),
                                        (
                                            byte_stream,
                                            0,
                                            decrypt_offset + take as u64,
                                            new_delivered,
                                            done,
                                            permit,
                                        ),
                                    ));
                                }

                                // normal path: skip already done
                                let remaining = expected_len.saturating_sub(delivered) as usize;
                                let take = remaining.min(chunk.len());
                                let mut output = chunk[..take].to_vec();
                                cipher.decrypt(decrypt_offset as usize, &mut output);
                                let new_delivered = delivered + take as u64;
                                let done = new_delivered == expected_len;
                                return Some((
                                    Ok(output),
                                    (
                                        byte_stream,
                                        0,
                                        decrypt_offset + take as u64,
                                        new_delivered,
                                        done,
                                        permit,
                                    ),
                                ));
                            }
                            Some(Err(e)) => {
                                return Some((
                                    Err(io::Error::other(format!("stream read error: {e}"))),
                                    (byte_stream, 0, decrypt_offset, delivered, true, permit),
                                ));
                            }
                            None => {
                                if delivered == expected_len {
                                    return None;
                                }
                                return Some((
                                    Err(io::Error::other(
                                        "CDN body ended before the requested range",
                                    )),
                                    (byte_stream, 0, decrypt_offset, delivered, true, permit),
                                ));
                            }
                        }
                    }
                }
            },
        )))
    } else {
        Err(io::Error::other(format!(
            "CDN returned unexpected status: {status}"
        )))
    }
}

// ── 探测与就绪 ──────────────────────────────────────────────────────

/// 最大尾部探测大小。
const TAIL_PROBE: u64 = 0x40;

/// CDN 请求客户端：reqwest 默认无任何超时，连接被防火墙黑洞时取流会
/// 永久挂起（无错误、无回退），流式期间还会占死 Semaphore 许可饿死其余
/// 区间请求。连接超时 + 读超时兜住这类故障。
///
/// **直连不走系统代理**：QQ CDN（isure/y.gtimg 等均为国内 CDN）直连稳定
/// 可达，而走系统代理（Clash 等）时大流量长流会被中间件随机停摆——小
/// 区间探测正常、流中途断流，且故障无错误无回退（2026-09-29 QQ 远端
/// 音频无法播放事故）。媒体取流路径不依赖用户的代理中间件。
///
/// `pub`：daemon 侧 QQ 封面下载（ContentService）复用同一超时配置。
pub fn cdn_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("CDN client build is infallible")
}

/// 将 CDN URL 准备为进程内随机访问解密源。
///
/// 流程：
/// 1. 探测 CDN 是否支持 Range（HEAD + GET Range: bytes=0-0）
/// 2. 若不支持 → 回退到 `decrypt::prepare_playable_at` /
///    `decrypt::prepare_playable_embedded_at`（`file://`，`source=None`）
/// 3. 拉尾部 → `detect_footer` → 派生 `ekey` → 构建 `StreamSource`
///    （ekey 为空且无 footer → 明文直通 `IdentityCipher`）
/// 4. 返回 `PreparedMedia { uri: 原 url, source }`；流式路径总是武装
///    tee 边播边缓存（替代 daemon 原后台二次全量下载）
pub async fn prepare_media(
    url: &str,
    ekey: Option<&str>,
    progress: Option<&tokio::sync::watch::Sender<Option<f64>>>,
) -> Result<PreparedMedia, MediaError> {
    prepare_media_with_params(url, ekey, progress, ReaderParams::default()).await
}

/// [`prepare_media`] 的测试参数注入版。
pub(crate) async fn prepare_media_with_params(
    url: &str,
    ekey: Option<&str>,
    progress: Option<&tokio::sync::watch::Sender<Option<f64>>>,
    params: ReaderParams,
) -> Result<PreparedMedia, MediaError> {
    let ekey = ekey.filter(|e| !e.is_empty());
    let client = cdn_client();

    // 1. 探测 CDN Range 支持
    let total_len = match probe_cdn(&client, url).await {
        Ok(tl) => tl,
        Err(_) => {
            debug!("CDN probe failed; falling back to full download-decrypt-cache");
            return fallback_playable(url, ekey, progress).await;
        }
    };

    // 2. 拉尾部并检测 footer
    let tail_end = total_len.saturating_sub(1);
    let tail_start = total_len.saturating_sub(TAIL_PROBE);
    let mut tail_bytes = fetch_range(&client, url, tail_start, tail_end)
        .await
        .map_err(|e| MediaError::Network(format!("tail fetch failed: {e}")))?;

    let mut footer = qmc2::detect_footer(total_len as usize, &tail_bytes);

    // QTag/V1 → 检查是否需要拉精确尾部（ekey 文本区超出 0x40 窗口）
    let mut have_full_tail = false; // tail_bytes 是否覆盖 audio_len..end
    let needs_refetch =
        if let Some(Footer::QTag { audio_len: al } | Footer::V1 { audio_len: al }) = &footer {
            let al = *al as u64;
            al + 8 < total_len && (total_len - 8 - al) > TAIL_PROBE
        } else {
            false
        };
    if needs_refetch {
        let al = match &footer {
            Some(Footer::QTag { audio_len } | Footer::V1 { audio_len }) => *audio_len as u64,
            _ => unreachable!(),
        };
        debug!(
            audio_len = al,
            total_len, "ekey text region exceeds 0x40; fetching exact tail"
        );
        tail_bytes = fetch_range(&client, url, al, tail_end)
            .await
            .map_err(|e| MediaError::Network(format!("exact tail fetch failed: {e}")))?;
        // 用精确尾部重新检测
        footer = qmc2::detect_footer(al as usize + tail_bytes.len(), &tail_bytes);
        have_full_tail = true;
    }

    let audio_len: usize = match &footer {
        Some(Footer::QTag { audio_len: al } | Footer::V1 { audio_len: al }) => *al,
        None => total_len as usize,
    };

    // 3. 获取密钥
    let cipher: Arc<dyn Qmc2Cipher> = if let Some(e) = ekey {
        let c = qmc2::decrypt_factory(e).map_err(MediaError::Key)?;
        Arc::from(c)
    } else if footer.is_none() {
        // 明文音质：无 API ekey 且无 footer → 直通（audio_len == total_len）。
        // 不走内嵌 ekey 提取/回退——原逻辑此处会因提取失败而整体回退，
        // 明文曲目统一进入本函数后必须仍能流式开播。
        Arc::from(IdentityCipher)
    } else if have_full_tail {
        // 复用已拉取的精确尾部，避免重复请求
        match embedded_ekey_from_bytes(&tail_bytes, audio_len) {
            Ok(e) => {
                let c = qmc2::decrypt_factory(&e).map_err(MediaError::Key)?;
                Arc::from(c)
            }
            Err(_) => {
                warn!("embedded ekey extraction failed; falling back to full download");
                return fallback_playable(url, None, progress).await;
            }
        }
    } else {
        // 无 API ekey → 从尾部提取内嵌 ekey
        let ekey_tail = fetch_range(&client, url, audio_len as u64, tail_end)
            .await
            .map_err(|e| MediaError::Network(format!("ekey tail fetch failed: {e}")))?;

        match embedded_ekey_from_bytes(&ekey_tail, audio_len) {
            Ok(e) => {
                let c = qmc2::decrypt_factory(&e).map_err(MediaError::Key)?;
                Arc::from(c)
            }
            Err(_) => {
                warn!("embedded ekey extraction failed; falling back to full download");
                return fallback_playable(url, None, progress).await;
            }
        }
    };

    // 4. 构建 StreamSource：uri 保留原 CDN url（元数据），播放走进程内
    //    reader；tee 缓存键与回退/回填路径共用同一键空间
    let tee_plan = TeePlan {
        root: crate::default_cache_root()?,
        key: crate::cache::cache_key(url, ekey.unwrap_or("")),
    };
    let source = Arc::new(StreamSource::with_params(
        client,
        url,
        cipher,
        audio_len as u64,
        total_len,
        Some(tee_plan),
        params,
    ));

    Ok(PreparedMedia::streaming(url.to_owned(), source))
}

/// 探测 CDN 是否支持 Range 请求。
///
/// 先发 HEAD 拿正数 Content-Length；再发 `GET Range: bytes=0-0` 确认：
/// - 返回 206 且严格为 `Content-Range: bytes 0-0/{total}`
/// - `total` 为正数且与 HEAD Content-Length 一致
/// - 不满足 → 报错（触发回退）
async fn probe_cdn(client: &reqwest::Client, url: &str) -> Result<u64, MediaError> {
    // HEAD
    let head_resp = client
        .head(url)
        .send()
        .await
        .map_err(|e| MediaError::Network(format!("HEAD request failed: {e}")))?;

    let head_status = head_resp.status();
    if head_status != reqwest::StatusCode::OK {
        return Err(MediaError::HttpStatus(head_status.as_u16()));
    }

    let head_total = head_resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&total| total > 0)
        .ok_or_else(|| {
            MediaError::Unsupported("CDN HEAD missing a valid Content-Length".to_string())
        })?;

    // GET Range: bytes=0-0
    let range_resp = client
        .get(url)
        .header("Range", "bytes=0-0")
        .send()
        .await
        .map_err(|e| MediaError::Network(format!("range probe request failed: {e}")))?;

    let range_status = range_resp.status();
    if range_status != reqwest::StatusCode::PARTIAL_CONTENT {
        debug!(%range_status, "CDN Range 探测: 未返回 206");
        return Err(MediaError::Unsupported(
            "CDN does not support range requests".to_string(),
        ));
    }

    // 解析 Content-Range
    let total = range_resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_range_00)
        .ok_or_else(|| {
            MediaError::Unsupported("CDN did not return a strict Content-Range".to_string())
        })?;

    if total != head_total {
        return Err(MediaError::Unsupported(
            "CDN HEAD and range probe disagree on total length".to_string(),
        ));
    }

    debug!(total, "CDN Range 探测成功");
    Ok(total)
}

/// 严格解析 `Content-Range: bytes 0-0/{total}`，并要求 total 为正数。
fn parse_content_range_00(header: &str) -> Option<u64> {
    let total = header.strip_prefix("bytes 0-0/")?.parse::<u64>().ok()?;
    (total > 0).then_some(total)
}

/// 解析精确的 `Content-Range: bytes start-end/total`。
fn parse_content_range(header: &str) -> Option<(u64, u64, u64)> {
    let spec = header.strip_prefix("bytes ")?;
    let (range, total) = spec.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?, total.parse().ok()?))
}

/// 从 CDN 拉取指定区间。
async fn fetch_range(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    end: u64,
) -> io::Result<Vec<u8>> {
    let range_header = format!("bytes={start}-{end}");
    let response = client
        .get(url)
        .header("Range", &range_header)
        .send()
        .await
        .map_err(|e| io::Error::other(format!("range request failed: {e}")))?;

    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|e| io::Error::other(format!("range read failed: {e}")))?;

    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        Ok(body.to_vec())
    } else {
        Err(io::Error::other(format!(
            "CDN range request returned {status}"
        )))
    }
}

/// 回退到全量下载-解密-缓存流程（`file://` URI，无进程内 source）。
async fn fallback_playable(
    url: &str,
    ekey: Option<&str>,
    progress: Option<&tokio::sync::watch::Sender<Option<f64>>>,
) -> Result<PreparedMedia, MediaError> {
    let root = crate::default_cache_root()?;
    let uri = if let Some(e) = ekey {
        decrypt::prepare_playable_at(&root, url, Some(e), progress).await?
    } else {
        decrypt::prepare_playable_embedded_at(&root, url, progress).await?
    };

    Ok(PreparedMedia::fallback(uri))
}

// ── 测试 ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    use std::io::{Read, Seek as _, SeekFrom};

    use wiremock::matchers::{header_exists, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 解析 Range 头值，返回 `(start, end)`。
    fn parse_range_value(v: &str) -> Option<(u64, u64)> {
        let spec = v.strip_prefix("bytes=")?;
        let (start_str, end_str) = spec.split_once('-')?;
        let start: u64 = start_str.parse().ok()?;
        let end: u64 = end_str.parse().ok()?;
        Some((start, end))
    }

    /// 挂载支持 Range 的加密 CDN mock（动态 responder 按 Range 值返回
    /// 对应加密区间）。
    async fn setup_range_cdn(
        plaintext: &[u8],
        key: &[u8],
        with_footer: bool,
    ) -> (MockServer, String) {
        let (encrypted, ekey) = testutil::make_encrypted(plaintext, key, with_footer);
        let total_len = encrypted.len() as u64;

        let server = MockServer::start().await;

        // HEAD mock
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", total_len.to_string())
                    .insert_header("Accept-Ranges", "bytes"),
            )
            .mount(&server)
            .await;

        // 动态 Range mock：根据实际 Range 值返回对应数据
        Mock::given(method("GET"))
            .and(header_exists("Range"))
            .respond_with(move |req: &wiremock::Request| {
                let range_val = req
                    .headers
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");

                if let Some((start, end)) = parse_range_value(range_val) {
                    let end_capped = end.min(total_len.saturating_sub(1));
                    if start >= total_len {
                        return ResponseTemplate::new(416);
                    }
                    let body = &encrypted[start as usize..=end_capped as usize];
                    ResponseTemplate::new(206)
                        .insert_header(
                            "Content-Range",
                            format!("bytes {start}-{end_capped}/{total_len}"),
                        )
                        .set_body_bytes(body.to_vec())
                } else {
                    ResponseTemplate::new(416)
                }
            })
            .mount(&server)
            .await;

        (server, ekey)
    }

    /// 明文 CDN mock（IdentityCipher 路径）：Range 语义同上但 body 即明文。
    async fn setup_range_cdn_plain(plaintext: &[u8]) -> MockServer {
        let total_len = plaintext.len() as u64;
        let server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", total_len.to_string())
                    .insert_header("Accept-Ranges", "bytes"),
            )
            .mount(&server)
            .await;

        let body = plaintext.to_vec();
        Mock::given(method("GET"))
            .and(header_exists("Range"))
            .respond_with(move |req: &wiremock::Request| {
                let range_val = req
                    .headers
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if let Some((start, end)) = parse_range_value(range_val) {
                    let end_capped = end.min(total_len.saturating_sub(1));
                    if start >= total_len {
                        return ResponseTemplate::new(416);
                    }
                    ResponseTemplate::new(206)
                        .insert_header(
                            "Content-Range",
                            format!("bytes {start}-{end_capped}/{total_len}"),
                        )
                        .set_body_bytes(body[start as usize..=end_capped as usize].to_vec())
                } else {
                    ResponseTemplate::new(416)
                }
            })
            .mount(&server)
            .await;
        server
    }

    /// 从 prepared 打开 reader 并整体读出（blocking 线程：read 饥饿时
    /// 阻塞 condvar，不能占住测试 runtime 线程）。
    async fn read_all(prepared: &PreparedMedia) -> Vec<u8> {
        let source = prepared.source.as_ref().unwrap().clone();
        testutil::blocking(move || {
            let mut r = source.open().unwrap();
            let mut out = Vec::new();
            r.read_to_end(&mut out).unwrap();
            out
        })
        .await
    }

    // ── probe / 解析单元测试（原样保留） ──────────────────────────

    #[tokio::test]
    async fn probe_cdn_requires_head_content_length() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("Content-Range", "bytes 0-0/10")
                    .set_body_bytes(vec![0]),
            )
            .mount(&server)
            .await;

        assert!(
            probe_cdn(&reqwest::Client::new(), &server.uri())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn probe_cdn_rejects_mismatched_or_non_206_probe() {
        let mismatched = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(200).insert_header("Content-Length", "10"))
            .mount(&mismatched)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("Content-Range", "bytes 0-0/11")
                    .set_body_bytes(vec![0]),
            )
            .mount(&mismatched)
            .await;
        assert!(
            probe_cdn(&reqwest::Client::new(), &mismatched.uri())
                .await
                .is_err()
        );

        let ignored_range = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(200).insert_header("Content-Length", "10"))
            .mount(&ignored_range)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 10]))
            .mount(&ignored_range)
            .await;
        assert!(
            probe_cdn(&reqwest::Client::new(), &ignored_range.uri())
                .await
                .is_err()
        );
    }

    #[test]
    fn parse_content_range_00_is_strict() {
        assert_eq!(parse_content_range_00("bytes 0-0/10"), Some(10));
        assert_eq!(parse_content_range_00("bytes 0-1/10"), None);
        assert_eq!(parse_content_range_00("bytes 0-0/0"), None);
        assert_eq!(parse_content_range_00("bytes 0-0/10 extra"), None);
    }

    #[test]
    fn redact_url_strips_query() {
        assert_eq!(
            redact_url("https://cdn.example.com/a/b.mflac?guid=1&vkey=xyz"),
            "cdn.example.com/a/b.mflac"
        );
        // 非 URL 输入退化为去 query 的原串
        assert_eq!(redact_url("not a url?query"), "not a url");
    }

    // ── prepare_media / reader 集成测试 ───────────────────────────

    #[tokio::test]
    async fn prepare_media_serves_decrypted_range() {
        let _env = testutil::isolate_cache("decrypted_range");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..4096).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        // 总是附带 footer 以避免加密尾部被误判为 V1
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");

        assert_eq!(prepared.uri, server.uri(), "流式路径 uri 保留原 CDN url");
        let len = prepared.source.as_ref().unwrap().len();
        assert_eq!(
            len,
            plaintext.len() as u64,
            "len 应为剥离 footer 的 audio_len"
        );

        // seek 到 1000 读 1001 字节（seek 行为）
        let source = prepared.source.as_ref().unwrap().clone();
        let body = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            r.seek(SeekFrom::Start(1000)).unwrap();
            let mut buf = vec![0u8; 1001];
            r.read_exact(&mut buf).unwrap();
            buf
        })
        .await;
        assert_eq!(&body, &plaintext[1000..=2000]);

        // 回到 0 读 4096 字节
        let source2 = prepared.source.as_ref().unwrap().clone();
        let body2 = testutil::blocking(move || {
            let mut r = source2.open().unwrap();
            let mut buf = vec![0u8; 4096];
            r.read_exact(&mut buf).unwrap();
            buf
        })
        .await;
        assert_eq!(&body2, &plaintext[..4096]);
    }

    #[tokio::test]
    async fn prepare_media_open_ended_full_read() {
        let _env = testutil::isolate_cache("open_ended");
        let plaintext = {
            let mut v = b"OggS".to_vec();
            v.extend((0..8192).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");

        // 顺序读全量（原开放区间 bytes=0- 语义）
        let body = read_all(&prepared).await;
        assert_eq!(body.len(), plaintext.len());
        assert_eq!(&body, &plaintext);
    }

    #[tokio::test]
    async fn prepare_media_seek_clamps_beyond_end() {
        let _env = testutil::isolate_cache("seek_clamp");
        let plaintext = b"fLaC test data".to_vec();
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");

        let source = prepared.source.as_ref().unwrap().clone();
        let want_len = plaintext.len() as u64;
        let (pos, n) = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            let pos = r.seek(SeekFrom::Start(want_len + 999)).unwrap();
            let mut buf = [0u8; 8];
            (pos, r.read(&mut buf).unwrap())
        })
        .await;
        // 原 416 语义 → clamp 到 [0, len]：返回 len，读返回 0
        assert_eq!(pos, plaintext.len() as u64);
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn prepare_media_caps_at_audio_len() {
        let _env = testutil::isolate_cache("caps_at_len");
        let plaintext = b"fLaC caps".to_vec();
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");

        // bytes=0- 语义：读全量长度应 == audio_len（plaintext 长度），无 footer 字节
        let body = read_all(&prepared).await;
        assert_eq!(body.len(), plaintext.len());
        assert_eq!(&body, &plaintext);

        // 末尾 4 字节（文件仅 9 字节，取小值防下溢）
        let source = prepared.source.as_ref().unwrap().clone();
        let tail = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            r.seek(SeekFrom::End(-4)).unwrap();
            let mut buf = Vec::new();
            r.read_to_end(&mut buf).unwrap();
            buf
        })
        .await;
        assert_eq!(&tail, &plaintext[plaintext.len() - 4..]);
    }

    #[tokio::test]
    async fn prepare_media_falls_back_without_cdn_range() {
        let _env = testutil::isolate_cache("fallback");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..1024).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (encrypted, ekey) = testutil::make_encrypted(&plaintext, key, false);

        let server = MockServer::start().await;

        // 不 mount HEAD mock（导致 probe_cdn 失败 → 回退）
        // 回退路径用 GET 下载全量
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encrypted.clone()))
            .mount(&server)
            .await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 回退应成功");

        // 应返回 file:// URI，source 为 None
        assert!(
            prepared.uri.starts_with("file://"),
            "expected file:// URI, got {}",
            prepared.uri
        );
        assert!(prepared.source.is_none(), "回退路径无进程内 source");

        // 验证内容一致
        let path = url::Url::parse(&prepared.uri)
            .unwrap()
            .to_file_path()
            .unwrap();
        let decoded = std::fs::read(path).unwrap();
        assert_eq!(decoded, plaintext, "回退内容应与明文一致");
    }

    #[tokio::test]
    async fn prepare_media_two_readers_share_source() {
        let _env = testutil::isolate_cache("two_readers");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..2048).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");

        // 同一 source 打开两个 reader（`open` 可重复调用）
        let s1 = prepared.source.as_ref().unwrap().clone();
        let body1 = testutil::blocking(move || {
            let mut r = s1.open().unwrap();
            let mut buf = vec![0u8; 100];
            r.read_exact(&mut buf).unwrap();
            buf
        })
        .await;
        assert_eq!(&body1, &plaintext[0..100]);

        let s2 = prepared.source.as_ref().unwrap().clone();
        let body2 = testutil::blocking(move || {
            let mut r = s2.open().unwrap();
            r.seek(SeekFrom::Start(100)).unwrap();
            let mut buf = vec![0u8; 100];
            r.read_exact(&mut buf).unwrap();
            buf
        })
        .await;
        assert_eq!(&body2, &plaintext[100..200]);
    }

    #[tokio::test]
    async fn prepare_media_embedded_ekey_from_tail() {
        let _env = testutil::isolate_cache("embedded_ekey");
        let plaintext = b"fLaC embedded proxy qtag";
        let key = b"0123456789abcdefghij";
        let (mut encrypted, ekey) = testutil::make_encrypted(plaintext, key, false);

        // 附加 QTag 尾部
        let metadata = format!("{ekey},123,2,");
        let payload_size = metadata.len() as u32;
        encrypted.extend_from_slice(metadata.as_bytes());
        encrypted.extend_from_slice(&payload_size.to_be_bytes());
        encrypted.extend_from_slice(b"QTag");

        let total_len = encrypted.len() as u64;

        let server = MockServer::start().await;

        // 单个 mock 处理所有请求（HEAD + GET Range），避免匹配顺序问题
        Mock::given(wiremock::matchers::any())
            .respond_with(move |req: &wiremock::Request| {
                if req.method == "HEAD" {
                    return ResponseTemplate::new(200)
                        .insert_header("Content-Length", total_len.to_string())
                        .insert_header("Accept-Ranges", "bytes");
                }

                if let Some(range_val) = req.headers.get("Range").and_then(|v| v.to_str().ok()) {
                    if let Some((start, end)) = parse_range_value(range_val) {
                        let end_capped = end.min(total_len.saturating_sub(1));
                        if start >= total_len {
                            return ResponseTemplate::new(416);
                        }
                        let body = &encrypted[start as usize..=end_capped as usize];
                        return ResponseTemplate::new(206)
                            .insert_header(
                                "Content-Range",
                                format!("bytes {start}-{end_capped}/{total_len}"),
                            )
                            .set_body_bytes(body.to_vec());
                    }
                }

                ResponseTemplate::new(200).set_body_bytes(encrypted.clone())
            })
            .mount(&server)
            .await;

        // 无 API ekey → 从 QTag 尾部提取
        let prepared = prepare_media(&server.uri(), None, None)
            .await
            .expect("prepare_media 应成功（无 API ekey）");
        assert!(prepared.source.is_some());

        // 全量读验证内容一致
        let body = read_all(&prepared).await;
        assert_eq!(&body, plaintext);
    }

    #[tokio::test]
    async fn prepare_media_seek_back_after_forward() {
        let _env = testutil::isolate_cache("seek_back");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..8192).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();

        let (body1, reader) = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            r.seek(SeekFrom::Start(5000)).unwrap();
            let mut buf = vec![0u8; 1001];
            r.read_exact(&mut buf).unwrap();
            (buf, r)
        })
        .await;
        assert_eq!(&body1, &plaintext[5000..=6000]);

        // 再 backward seek（无状态污染）
        let body2 = testutil::blocking(move || {
            let mut reader = reader;
            reader.seek(SeekFrom::Start(0)).unwrap();
            let mut buf = vec![0u8; 1001];
            reader.read_exact(&mut buf).unwrap();
            buf
        })
        .await;
        assert_eq!(&body2, &plaintext[0..=1000]);
    }

    #[tokio::test]
    async fn prepare_media_read_err_on_invalid_runtime_206() {
        let _env = testutil::isolate_cache("invalid_206");
        // 运行时 206 Content-Range 违约 → Read Err（原 502）
        let plaintext = b"fLaC invalid response".to_vec();
        let key = b"0123456789abcdefghij";
        let (encrypted, ekey) = testutil::make_encrypted(&plaintext, key, true);
        let total_len = encrypted.len() as u64;
        let server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200).insert_header("Content-Length", total_len.to_string()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header_exists("Range"))
            .respond_with(move |req: &wiremock::Request| {
                let range = req
                    .headers
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if range == "bytes=0-0" {
                    return ResponseTemplate::new(206)
                        .insert_header("Content-Range", format!("bytes 0-0/{total_len}"))
                        .set_body_bytes(encrypted[0..=0].to_vec());
                }
                if let Some((start, end)) = parse_range_value(range) {
                    // 尾部探测：请求穿透到文件末字节（prepare_media 的
                    // footer 探测）；数据区间止于 audio_len-1，不含 footer
                    if end == total_len - 1 {
                        let end_capped = end.min(total_len - 1);
                        return ResponseTemplate::new(206)
                            .insert_header(
                                "Content-Range",
                                format!("bytes {start}-{end_capped}/{total_len}"),
                            )
                            .set_body_bytes(
                                encrypted[start as usize..=end_capped as usize].to_vec(),
                            );
                    }
                    // 数据区间：Content-Range 与请求不符（违约）
                    return ResponseTemplate::new(206)
                        .insert_header("Content-Range", format!("bytes 1-1/{total_len}"))
                        .set_body_bytes(encrypted[0..1].to_vec());
                }
                ResponseTemplate::new(416)
            })
            .mount(&server)
            .await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();
        let err = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            let mut buf = vec![0u8; 4];
            r.read_exact(&mut buf).err()
        })
        .await;
        assert!(err.is_some(), "违约 206 应让 Read 返回 Err（原 502 语义）");
    }

    #[tokio::test]
    async fn prepare_media_read_err_on_short_runtime_206_body() {
        let _env = testutil::isolate_cache("short_206");
        // 运行时 206 body 短于声明区间 → Read Err（原 502）
        let plaintext = b"fLaC short response".to_vec();
        let key = b"0123456789abcdefghij";
        let (encrypted, ekey) = testutil::make_encrypted(&plaintext, key, true);
        let total_len = encrypted.len() as u64;
        let server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200).insert_header("Content-Length", total_len.to_string()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header_exists("Range"))
            .respond_with(move |req: &wiremock::Request| {
                let range = req
                    .headers
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if let Some((start, end)) = parse_range_value(range) {
                    let end_capped = end.min(total_len - 1);
                    // 尾部探测：请求穿透到文件末字节
                    if end == total_len - 1 {
                        return ResponseTemplate::new(206)
                            .insert_header(
                                "Content-Range",
                                format!("bytes {start}-{end_capped}/{total_len}"),
                            )
                            .set_body_bytes(
                                encrypted[start as usize..=end_capped as usize].to_vec(),
                            );
                    }
                    // 数据区间：声明全区间但 body 只给 1 字节
                    return ResponseTemplate::new(206)
                        .insert_header(
                            "Content-Range",
                            format!("bytes {start}-{end_capped}/{total_len}"),
                        )
                        .set_body_bytes(encrypted[start as usize..=start as usize].to_vec());
                }
                ResponseTemplate::new(416)
            })
            .mount(&server)
            .await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();
        let err = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            let mut all = Vec::new();
            r.read_to_end(&mut all).err()
        })
        .await;
        assert!(
            err.is_some(),
            "短 body 206 应让 Read 返回 Err（原 502 语义）"
        );
    }

    #[tokio::test]
    async fn prepare_media_read_err_on_200_short_body() {
        let _env = testutil::isolate_cache("200_short");
        // 200 防御路径：CDN body 远短于请求的 start，skip 耗尽后报错 →
        // Read Err（原 502）
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..512).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (encrypted, ekey) = testutil::make_encrypted(&plaintext, key, true);
        let total_len = encrypted.len() as u64;
        // body 仅 50 字节，远小于请求的 start（100），全部被 skip 消耗
        let encrypted_short = encrypted[..50].to_vec();

        let server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", total_len.to_string())
                    .insert_header("Accept-Ranges", "bytes"),
            )
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(header_exists("Range"))
            .respond_with(move |req: &wiremock::Request| {
                let range_val = req
                    .headers
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");

                if range_val == "bytes=0-0" {
                    return ResponseTemplate::new(206)
                        .insert_header("Content-Range", format!("bytes 0-0/{total_len}"))
                        .set_body_bytes(encrypted[0..=0].to_vec());
                }

                if let Some((start, end)) = parse_range_value(range_val) {
                    if start >= total_len.saturating_sub(0x40) {
                        let end_capped = end.min(total_len.saturating_sub(1));
                        let body = &encrypted[start as usize..=end_capped as usize];
                        return ResponseTemplate::new(206)
                            .insert_header(
                                "Content-Range",
                                format!("bytes {start}-{end_capped}/{total_len}"),
                            )
                            .set_body_bytes(body.to_vec());
                    }
                }

                // 200 但 body 极短
                ResponseTemplate::new(200).set_body_bytes(encrypted_short.clone())
            })
            .mount(&server)
            .await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();
        let err = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            r.seek(SeekFrom::Start(100)).unwrap();
            let mut buf = vec![0u8; 100];
            r.read_exact(&mut buf).err()
        })
        .await;
        assert!(err.is_some(), "start=100 > 短 body 50 字节 → Read Err");
    }

    #[tokio::test]
    async fn prepare_media_200_defense_path() {
        let _env = testutil::isolate_cache("200_defense");
        // CDN probe 返回 206，但数据区间请求返回 200 全量 body：
        // 生产者必须流式跳过 start 之前的字节并正确解密 [start..=end]
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..512).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (encrypted, ekey) = testutil::make_encrypted(&plaintext, key, true);
        let total_len = encrypted.len() as u64;
        let encrypted_full = encrypted.clone();

        let server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", total_len.to_string())
                    .insert_header("Accept-Ranges", "bytes"),
            )
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(header_exists("Range"))
            .respond_with(move |req: &wiremock::Request| {
                let range_val = req
                    .headers
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");

                if range_val == "bytes=0-0" {
                    let body = &encrypted[0..=0];
                    return ResponseTemplate::new(206)
                        .insert_header("Content-Range", format!("bytes 0-0/{total_len}"))
                        .set_body_bytes(body.to_vec());
                }

                if let Some((start, end)) = parse_range_value(range_val) {
                    // 尾部区间返回 206（prepare_media 需要检测 footer）
                    if start >= total_len.saturating_sub(0x40) {
                        let end_capped = end.min(total_len.saturating_sub(1));
                        let body = &encrypted_full[start as usize..=end_capped as usize];
                        return ResponseTemplate::new(206)
                            .insert_header(
                                "Content-Range",
                                format!("bytes {start}-{end_capped}/{total_len}"),
                            )
                            .set_body_bytes(body.to_vec());
                    }
                }

                // 其他区间 → 200 全量（CDN 忽略 Range，触发 200 防御路径）
                ResponseTemplate::new(200).set_body_bytes(encrypted_full.clone())
            })
            .mount(&server)
            .await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();
        let body = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            r.seek(SeekFrom::Start(100)).unwrap();
            let mut buf = vec![0u8; 100];
            r.read_exact(&mut buf).unwrap();
            buf
        })
        .await;
        assert_eq!(&body, &plaintext[100..=199], "200 跳过路径应正确解密");
    }

    // ── reader 新增行为 ────────────────────────────────────────────

    #[tokio::test]
    async fn reader_chunked_sequential_reads_equal() {
        let _env = testutil::isolate_cache("chunked_reads");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..1000).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_media(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();

        // 以 7 字节小缓冲逐块读全量（分块边界无伪影）
        let body = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            let mut out = Vec::new();
            let mut buf = [0u8; 7];
            loop {
                let n = r.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..n]);
            }
            out
        })
        .await;
        assert_eq!(&body, &plaintext, "分块顺序读必须与明文全等");
    }

    #[tokio::test]
    async fn reader_in_window_seek_back_no_cdn() {
        let _env = testutil::isolate_cache("in_window_seek");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..252).map(|i| (i % 256) as u8)); // 总长 256
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;
        let (encrypted, _) = testutil::make_encrypted(&plaintext, key, true);
        let tail_from = encrypted.len() as u64 - 0x40;

        // 小窗口参数：chunk=64（256 → 4 窗口全量拉满即 eof），retain=512
        // （全程不修剪 → 窗口始终覆盖 [0, 256)）
        let params = super::ReaderParams {
            chunk: 64,
            prefetch: 4096,
            retain: 512,
        };
        let prepared = prepare_media_with_params(&server.uri(), Some(&ekey), None, params)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();

        // 先打开 reader（生产者随 open 启动）并读出前 128 字节
        let (first, reader) = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            let mut buf = vec![0u8; 128];
            r.read_exact(&mut buf).unwrap();
            (buf, r)
        })
        .await;
        assert_eq!(&first, &plaintext[..128]);

        // 等待生产者拉满 eof（256 字节 / 64 字节窗口 = 4 个数据请求，
        // 此后请求数稳定）
        testutil::eventually_async("4 个数据窗口拉满", || async {
            let c = testutil::data_get_count(&server, tail_from).await;
            (c == 4).then_some(c)
        })
        .await;

        // 窗口内回看：seek(0) 后重读，不应触发任何新的 CDN 请求
        let second = testutil::blocking(move || {
            let mut reader = reader;
            reader.seek(SeekFrom::Start(0)).unwrap();
            let mut buf = vec![0u8; 128];
            reader.read_exact(&mut buf).unwrap();
            buf
        })
        .await;
        assert_eq!(&second, &plaintext[..128]);

        let count = testutil::data_get_count(&server, tail_from).await;
        assert_eq!(count, 4, "窗口内回看不应触发新 CDN 请求");
    }

    #[tokio::test]
    async fn reader_out_of_window_seek_refetches() {
        let _env = testutil::isolate_cache("out_window_seek");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..1020).map(|i| (i % 256) as u8)); // 总长 1024
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;
        let (encrypted, _) = testutil::make_encrypted(&plaintext, key, true);
        let tail_from = encrypted.len() as u64 - 0x40;

        // retain=0（读完即弃）、prefetch=128：fetched_until 最多停在
        // pos+prefetch+chunk ≈ 320，900 必在窗口外
        let params = super::ReaderParams {
            chunk: 64,
            prefetch: 128,
            retain: 0,
        };
        let prepared = prepare_media_with_params(&server.uri(), Some(&ekey), None, params)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();

        let (prefix, mut reader) = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            let mut buf = vec![0u8; 128];
            r.read_exact(&mut buf).unwrap();
            (buf, r)
        })
        .await;
        assert_eq!(&prefix, &plaintext[..128]);
        let c1 = testutil::data_get_count(&server, tail_from).await;

        // 窗口外 seek → 生产者在目标偏移重启拉取
        let body = testutil::blocking(move || {
            reader.seek(SeekFrom::Start(900)).unwrap();
            let mut buf = vec![0u8; 64];
            reader.read_exact(&mut buf).unwrap();
            buf
        })
        .await;
        assert_eq!(&body, &plaintext[900..964]);

        let c2 = testutil::data_get_count(&server, tail_from).await;
        assert!(c2 > c1, "窗口外 seek 应重新拉取 CDN（{c1} → {c2}）");
    }

    #[tokio::test]
    async fn reader_drop_stops_producer() {
        let _env = testutil::isolate_cache("drop_stops");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..4092).map(|i| (i % 256) as u8)); // 总长 4096 → 64 窗口
            v
        };
        let key = b"0123456789abcdefghij";
        let (encrypted, ekey) = testutil::make_encrypted(&plaintext, key, true);
        let total_len = encrypted.len() as u64;
        let tail_from = total_len - 0x40;

        // 响应统一加 10ms 延迟：64 个窗口串行拉完需 ~640ms，读 8 字节即
        // drop 后生产者必须停下（prefetch 无上限，若不取消会拉完全部）
        let server = MockServer::start().await;
        let delay = std::time::Duration::from_millis(10);
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", total_len.to_string())
                    .set_delay(delay),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header_exists("Range"))
            .respond_with(move |req: &wiremock::Request| {
                let range_val = req
                    .headers
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                let resp = if let Some((start, end)) = parse_range_value(range_val) {
                    let end_capped = end.min(total_len.saturating_sub(1));
                    if start >= total_len {
                        ResponseTemplate::new(416)
                    } else {
                        ResponseTemplate::new(206)
                            .insert_header(
                                "Content-Range",
                                format!("bytes {start}-{end_capped}/{total_len}"),
                            )
                            .set_body_bytes(
                                encrypted[start as usize..=end_capped as usize].to_vec(),
                            )
                    }
                } else {
                    ResponseTemplate::new(416)
                };
                resp.set_delay(delay)
            })
            .mount(&server)
            .await;

        let params = super::ReaderParams {
            chunk: 64,
            prefetch: u64::MAX, // 从不因预取上限暂停
            retain: 512,
        };
        let prepared = prepare_media_with_params(&server.uri(), Some(&ekey), None, params)
            .await
            .expect("prepare_media 应成功");
        let concrete = prepared.concrete.as_ref().unwrap().clone();

        let (reader, watch) = testutil::blocking(move || {
            let mut r = concrete.open_reader().unwrap();
            let mut buf = [0u8; 8];
            r.read_exact(&mut buf).unwrap();
            let w = r.producer_watch();
            (r, w)
        })
        .await;
        drop(reader);

        // 等生产者任务真正退出后再断言计数静止
        testutil::eventually("生产者退出", || watch.exited().then_some(())).await;
        let c1 = testutil::data_get_count(&server, tail_from).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let c2 = testutil::data_get_count(&server, tail_from).await;
        assert_eq!(c2, c1, "drop 后生产者必须停止（计数静止）");
        assert!(c1 < 10, "读 8 字节即 drop，只应发生少量窗口请求，got {c1}");
    }

    #[tokio::test]
    async fn prepare_media_plaintext_identity() {
        let _env = testutil::isolate_cache("plaintext");
        // 明文（无 ekey 且尾部不构成 footer）→ IdentityCipher 直通
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..2000).map(|i| (i % 256) as u8));
            v
        };
        // 尾部 4 字节 [204,205,206,207] 的 LE u32 远超 V1 key 上限 0x400，
        // detect_footer → None（不会误判进入内嵌 ekey 提取）
        let server = setup_range_cdn_plain(&plaintext).await;
        let url = format!("{}/song.flac", server.uri());

        let prepared = prepare_media(&url, None, None)
            .await
            .expect("明文应走流式直通而非回退");
        assert!(prepared.source.is_some(), "明文应提供进程内 source");
        assert_eq!(prepared.uri, url);

        let body = read_all(&prepared).await;
        assert_eq!(&body, &plaintext, "明文直通内容必须全等");

        // tee 同样生效：二次播放经 cached_playable_uri 全离线命中
        let hit = testutil::eventually("明文 tee 缓存完成", || {
            crate::cached_playable_uri(&url, None).unwrap()
        })
        .await;
        let path = url::Url::parse(&hit).unwrap().to_file_path().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), plaintext);
    }

    // ── tee 边播边缓存 ─────────────────────────────────────────────

    /// 读前缀 + drop → 后台补齐 → cached_playable_uri 命中且内容与参考
    /// 解密一致。
    #[tokio::test]
    async fn tee_read_prefix_drop_completes() {
        let _env = testutil::isolate_cache("tee_prefix");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..4096).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;
        let url = format!("{}/song.mflac", server.uri());

        let prepared = prepare_media(&url, Some(&ekey), None)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();

        // 只读 100 字节即换曲（drop）
        testutil::blocking(move || {
            let mut r = source.open().unwrap();
            let mut buf = [0u8; 100];
            r.read_exact(&mut buf).unwrap();
        })
        .await;

        // drop 后后台补齐 [next_write, audio_len) 并收尾
        let hit = testutil::eventually("tee 后台补齐完成", || {
            crate::cached_playable_uri(&url, Some(&ekey)).unwrap()
        })
        .await;
        let path = url::Url::parse(&hit).unwrap().to_file_path().unwrap();
        assert_eq!(
            std::fs::read(path).unwrap(),
            plaintext,
            "补齐后的缓存内容应与参考解密全等"
        );
    }

    /// 前向 seek 跳洞 → tee 永久 detach；drop 后完成路径补齐（.tmp 不提前
    /// 转正），断言补齐后内容正确。
    #[tokio::test]
    async fn tee_forward_seek_detach_completes() {
        let _env = testutil::isolate_cache("tee_detach");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..508).map(|i| (i % 256) as u8)); // 总长 512
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;
        let url = format!("{}/song.mflac", server.uri());

        // chunk=8 / prefetch=32 / retain=0：读 [0..16) 后 fetched_until ≤ 56，
        // seek(64) 必在窗口外 → 生产者跳到 64 → tee 遇跳洞永久 detach
        let params = super::ReaderParams {
            chunk: 8,
            prefetch: 32,
            retain: 0,
        };
        let prepared = prepare_media_with_params(&url, Some(&ekey), None, params)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();

        let (_, reader) = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            let mut buf = [0u8; 16];
            r.read_exact(&mut buf).unwrap();
            r.seek(SeekFrom::Start(64)).unwrap();
            let mut buf2 = [0u8; 16];
            r.read_exact(&mut buf2).unwrap();
            (buf2, r)
        })
        .await;
        drop(reader);

        // detach 的 tee：drop 时 tmp 不转正，由补齐任务拉满 [16, 512)
        let hit = testutil::eventually("tee 跳洞后补齐完成", || {
            crate::cached_playable_uri(&url, Some(&ekey)).unwrap()
        })
        .await;
        let path = url::Url::parse(&hit).unwrap().to_file_path().unwrap();
        assert_eq!(
            std::fs::read(path).unwrap(),
            plaintext,
            "detach 后补齐的缓存内容应完整正确"
        );
    }

    /// 探测期回读（窗口外 seek 回拉已写区间）→ tee 跳过不写也不 detach：
    /// 生产者追上 next_write 后继续追加，读穿后 drop 直接收尾（无补齐请求）。
    #[tokio::test]
    async fn tee_probe_reread_does_not_detach() {
        let _env = testutil::isolate_cache("tee_reread");
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..508).map(|i| (i % 256) as u8)); // 总长 512
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;
        let url = format!("{}/song.mflac", server.uri());
        let (encrypted, _) = testutil::make_encrypted(&plaintext, key, true);
        let tail_from = encrypted.len() as u64 - 0x40;

        let params = super::ReaderParams {
            chunk: 8,
            prefetch: 32,
            retain: 0,
        };
        let prepared = prepare_media_with_params(&url, Some(&ekey), None, params)
            .await
            .expect("prepare_media 应成功");
        let source = prepared.source.as_ref().unwrap().clone();

        // 读 [0..16) → tee 写到 16；seek(8) 窗口外回读 → 生产者从 8 重拉
        // （tee 跳过 [8,16) 不 detach）；随后读穿到结尾（tee 追加到 512）
        let expected = plaintext.clone();
        let (_, reader) = testutil::blocking(move || {
            let mut r = source.open().unwrap();
            let mut buf = [0u8; 16];
            r.read_exact(&mut buf).unwrap();
            assert_eq!(&buf, &expected[..16]);
            r.seek(SeekFrom::Start(8)).unwrap();
            let mut all = Vec::new();
            r.read_to_end(&mut all).unwrap();
            assert_eq!(&all, &expected[8..]);
            (all, r)
        })
        .await;
        let c1 = testutil::data_get_count(&server, tail_from).await;
        drop(reader);

        let hit = testutil::eventually("tee 读穿收尾完成", || {
            crate::cached_playable_uri(&url, Some(&ekey)).unwrap()
        })
        .await;
        let path = url::Url::parse(&hit).unwrap().to_file_path().unwrap();
        assert_eq!(
            std::fs::read(path).unwrap(),
            plaintext,
            "回读后 tee 应继续写入，最终缓存完整"
        );

        // 读穿 + 未 detach → drop 走直接收尾路径，不发生补齐请求
        let c2 = testutil::data_get_count(&server, tail_from).await;
        assert_eq!(c2, c1, "未 detach 的读穿 drop 不应触发补齐请求");
    }
}
