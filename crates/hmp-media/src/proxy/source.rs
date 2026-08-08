//! CDN 流式数据源：探测 CDN Range 支持 → 解析 QMC2 尾部 →
//! 构建流密码 → 按需拉取并分块解密区间。
//!
//! 若 CDN 不支持 `Range`（返回 200 或无 `Content-Range`），
//! 自动回退到 `decrypt` 全量下载-解密-缓存流程。
//!
//! 播放路径通过 [`Source::open`] 返回分块流，每个 chunk 在到达时
//! 即时解密并写入 TCP socket，避免全量缓冲。

use std::io;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::{Stream, StreamExt, TryStreamExt};
use hmp_qqmusic_api::algorithms::qmc2::{self, Footer, Qmc2Cipher};
use tokio::sync::{Semaphore, oneshot};
use tracing::{debug, warn};

use crate::MediaError;
use crate::decrypt;
use crate::decrypt::embedded_ekey_from_bytes;

use super::http::Source;
use super::range::ByteRange;

// ── 公共类型 ────────────────────────────────────────────────────────

/// 已就绪的流媒体端点。
///
/// `uri` 为 `http://127.0.0.1:<port>/stream`，或回退时 `file://...`。
/// 丢弃 [`PreparedMedia`] 会停止代理服务器。
pub struct PreparedMedia {
    /// 可播放的 URI（`http://` 或 `file://`）。
    pub uri: String,
    /// 停止信号持有者；drop 时通知 `serve` 退出 accept 循环。
    pub _guard: MediaGuard,
}

/// Drop 时向代理服务器发送停止信号。
///
/// 若 `guard` 为 `Some(tx)`，drop 时发送 `()`。
/// `serve` 收到信号后退出 accept 循环并释放监听端口。
pub struct MediaGuard {
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for MediaGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

// ── 流式数据源 ──────────────────────────────────────────────────────

/// CDN 区间拉取 + 按需 QMC2 解密的数据源。
///
/// 实现 [`Source`] 供 [`super::http::serve`] 使用。
/// [`open`](Source::open) 返回分块流，每块在到达时即时解密。
struct StreamSource {
    /// HTTP 客户端（复用连接）。
    client: reqwest::Client,
    /// CDN 地址。
    cdn_url: String,
    /// QMC2 流密码（解密区间数据）。
    cipher: Arc<dyn Qmc2Cipher>,
    /// 解密后的音频长度（剥离 footer 后）。
    audio_len: u64,
    /// CDN 上的原始文件总长。
    total_len: u64,
    /// 并发限制（最多 4 个并行 range 请求）。
    sem: Arc<Semaphore>,
}

impl Source for StreamSource {
    fn audio_len(&self) -> u64 {
        self.audio_len
    }

    type ChunkStream<'a> = Pin<Box<dyn Stream<Item = io::Result<Vec<u8>>> + Send + 'a>>;

    fn open<'a>(&'a self, range: ByteRange) -> Self::ChunkStream<'a> {
        let client = self.client.clone();
        let cdn_url = self.cdn_url.clone();
        let cipher = Arc::clone(&self.cipher);
        let sem = Arc::clone(&self.sem);
        let total_len = self.total_len;

        // 此 future 由 HTTP 连接任务直接驱动；连接取消时请求、等待信号量和
        // 解密链都会一同 drop，不会留下后台 producer。
        Box::pin(
            futures_util::stream::once(async move {
                fetch_and_decrypt_range(client, cdn_url, cipher, sem, range, total_len).await
            })
            .try_flatten(),
        )
    }
}

type OwnedChunkStream = Pin<Box<dyn Stream<Item = io::Result<Vec<u8>>> + Send>>;

/// 拉取并验证一个区间。返回的流由 HTTP 连接任务直接驱动。
async fn fetch_and_decrypt_range(
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
        .map_err(|e| io::Error::other(format!("信号量获取失败: {e}")))?;
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
        .map_err(|e| io::Error::other(format!("CDN 请求失败: {e}")))?;
    let status = response.status();

    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        let valid_range = response
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| parse_content_range(v) == Some((start, end, total_len)));
        if !valid_range {
            return Err(io::Error::other("CDN 206 Content-Range 与请求不一致"));
        }
        if response
            .content_length()
            .is_some_and(|len| len != expected_len)
        {
            return Err(io::Error::other("CDN 206 body 长度与请求区间不一致"));
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
                                    Err(io::Error::other("CDN 206 body 超出请求区间")),
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
                            Err(io::Error::other(format!("读取流错误: {e}"))),
                            (byte_stream, offset, delivered, true, permit),
                        )),
                        None if delivered == expected_len => None,
                        None => Some((
                            Err(io::Error::other("CDN 206 body 在请求区间前结束")),
                            (byte_stream, offset, delivered, true, permit),
                        )),
                    }
                }
            },
        )))
    } else if status == reqwest::StatusCode::OK {
        warn!(cdn_url = %cdn_url, "CDN 返回 200（忽略 Range），流式跳过");
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
                                    Err(io::Error::other(format!("读取流错误: {e}"))),
                                    (byte_stream, 0, decrypt_offset, delivered, true, permit),
                                ));
                            }
                            None => {
                                if delivered == expected_len {
                                    return None;
                                }
                                return Some((
                                    Err(io::Error::other("CDN body 在请求区间前结束")),
                                    (byte_stream, 0, decrypt_offset, delivered, true, permit),
                                ));
                            }
                        }
                    }
                }
            },
        )))
    } else {
        Err(io::Error::other(format!("CDN 返回非预期状态码: {status}")))
    }
}

// ── 探测与就绪 ──────────────────────────────────────────────────────

/// 最大尾部探测大小。
const TAIL_PROBE: u64 = 0x40;

/// 将 CDN URL 准备为流式代理。
///
/// 流程：
/// 1. 探测 CDN 是否支持 Range（HEAD + GET Range: bytes=0-0）
/// 2. 若不支持 → 回退到 `decrypt::prepare_playable_at` /
///    `decrypt::prepare_playable_embedded_at`
/// 3. 拉尾部 → `detect_footer` → 派生 `ekey` → 构建 `StreamSource`
/// 4. 在 `127.0.0.1:0` 启动 HTTP 代理，返回 `PreparedMedia`
pub async fn prepare_stream(
    url: &str,
    ekey: Option<&str>,
    progress: Option<&tokio::sync::watch::Sender<Option<f64>>>,
) -> Result<PreparedMedia, MediaError> {
    let ekey = ekey.filter(|e| !e.is_empty());
    let client = reqwest::Client::new();

    // 1. 探测 CDN Range 支持
    let total_len = match probe_cdn(&client, url).await {
        Ok(tl) => tl,
        Err(_) => {
            debug!("CDN 探测失败，回退到全量下载-解密-缓存");
            return fallback_playable(url, ekey, progress).await;
        }
    };

    // 2. 拉尾部并检测 footer
    let tail_end = total_len.saturating_sub(1);
    let tail_start = total_len.saturating_sub(TAIL_PROBE);
    let mut tail_bytes = fetch_range(&client, url, tail_start, tail_end)
        .await
        .map_err(|e| MediaError::Network(format!("尾部拉取失败: {e}")))?;

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
            total_len, "ekey 文本区超出 0x40，拉取精确尾部"
        );
        tail_bytes = fetch_range(&client, url, al, tail_end)
            .await
            .map_err(|e| MediaError::Network(format!("精确尾部拉取失败: {e}")))?;
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
    } else if have_full_tail {
        // 复用已拉取的精确尾部，避免重复请求
        match embedded_ekey_from_bytes(&tail_bytes, audio_len) {
            Ok(e) => {
                let c = qmc2::decrypt_factory(&e).map_err(MediaError::Key)?;
                Arc::from(c)
            }
            Err(_) => {
                warn!("内嵌 ekey 提取失败，回退到全量下载");
                return fallback_playable(url, None, progress).await;
            }
        }
    } else {
        // 无 API ekey → 从尾部提取内嵌 ekey
        let ekey_tail = fetch_range(&client, url, audio_len as u64, tail_end)
            .await
            .map_err(|e| MediaError::Network(format!("ekey 尾部拉取失败: {e}")))?;

        match embedded_ekey_from_bytes(&ekey_tail, audio_len) {
            Ok(e) => {
                let c = qmc2::decrypt_factory(&e).map_err(MediaError::Key)?;
                Arc::from(c)
            }
            Err(_) => {
                warn!("内嵌 ekey 提取失败，回退到全量下载");
                return fallback_playable(url, None, progress).await;
            }
        }
    };

    // 4. 构建 StreamSource 并启动代理
    let source = Arc::new(StreamSource {
        client: client.clone(),
        cdn_url: url.to_owned(),
        cipher,
        audio_len: audio_len as u64,
        total_len,
        sem: Arc::new(Semaphore::new(4)),
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(MediaError::Io)?;
    let addr = listener.local_addr().map_err(MediaError::Io)?;
    let port = addr.port();

    let (stop_tx, stop_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        super::http::serve(listener, source, stop_rx).await;
    });

    Ok(PreparedMedia {
        uri: format!("http://127.0.0.1:{port}/stream"),
        _guard: MediaGuard {
            stop: Some(stop_tx),
        },
    })
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
        .map_err(|e| MediaError::Network(format!("HEAD 请求失败: {e}")))?;

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
        .ok_or_else(|| MediaError::Unsupported("CDN HEAD 缺少有效 Content-Length".to_string()))?;

    // GET Range: bytes=0-0
    let range_resp = client
        .get(url)
        .header("Range", "bytes=0-0")
        .send()
        .await
        .map_err(|e| MediaError::Network(format!("Range 探测请求失败: {e}")))?;

    let range_status = range_resp.status();
    if range_status != reqwest::StatusCode::PARTIAL_CONTENT {
        debug!(%range_status, "CDN Range 探测: 未返回 206");
        return Err(MediaError::Unsupported("CDN 不支持 Range 请求".to_string()));
    }

    // 解析 Content-Range
    let total = range_resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_range_00)
        .ok_or_else(|| MediaError::Unsupported("CDN 未返回严格的 Content-Range".to_string()))?;

    if total != head_total {
        return Err(MediaError::Unsupported(
            "CDN HEAD 与 Range 探测总长度不一致".to_string(),
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
        .map_err(|e| io::Error::other(format!("区间请求失败: {e}")))?;

    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|e| io::Error::other(format!("读取区间失败: {e}")))?;

    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        Ok(body.to_vec())
    } else {
        Err(io::Error::other(format!("CDN 区间请求返回 {status}")))
    }
}

/// 回退到全量下载-解密-缓存流程。
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

    Ok(PreparedMedia {
        uri,
        _guard: MediaGuard { stop: None },
    })
}

// ── 测试 ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    use tokio::net::TcpStream;
    use wiremock::matchers::{header_exists, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 通过 HTTP Range 请求从代理获取区间数据。
    async fn fetch_via_proxy(
        base_uri: &str,
        range_start: u64,
        range_end: Option<u64>,
    ) -> (u16, Vec<u8>) {
        let range_str = if let Some(end) = range_end {
            format!("bytes={range_start}-{end}")
        } else {
            format!("bytes={range_start}-")
        };

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{base_uri}/stream"))
            .header("Range", &range_str)
            .send()
            .await
            .expect("代理请求失败");

        let code = resp.status().as_u16();
        let body = resp.bytes().await.expect("读取代理响应失败").to_vec();
        (code, body)
    }

    /// 解析 Range 头值，返回 `(start, end)`。
    fn parse_range_value(v: &str) -> Option<(u64, u64)> {
        let spec = v.strip_prefix("bytes=")?;
        let (start_str, end_str) = spec.split_once('-')?;
        let start: u64 = start_str.parse().ok()?;
        let end: u64 = end_str.parse().ok()?;
        Some((start, end))
    }

    /// 设置支持 Range 的 CDN mock。
    ///
    /// 挂载一个动态 responder：根据实际 Range 值返回对应的加密区间数据。
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

    // ── 测试用例 ──────────────────────────────────────────────────

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

    #[tokio::test]
    async fn prepare_stream_serves_decrypted_range() {
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..4096).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        // 总是附带 footer 以避免加密尾部被误判为 V1
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");

        assert!(prepared.uri.starts_with("http://127.0.0.1:"));

        // 请求 bytes=0-4095
        let (code, body) = fetch_via_proxy(&prepared.uri, 0, Some(4095)).await;
        assert_eq!(code, 206);
        assert_eq!(&body, &plaintext[..4096]);

        // 请求 bytes=1000-2000（seek 行为）
        let (code2, body2) = fetch_via_proxy(&prepared.uri, 1000, Some(2000)).await;
        assert_eq!(code2, 206);
        assert_eq!(&body2, &plaintext[1000..=2000]);
    }

    #[tokio::test]
    async fn prepare_stream_open_ended_range() {
        let plaintext = {
            let mut v = b"OggS".to_vec();
            v.extend((0..8192).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");

        // bytes=0- → 全量明文（代理对 Range 请求一律返回 206）
        let (code, body) = fetch_via_proxy(&prepared.uri, 0, None).await;
        assert_eq!(code, 206);
        assert_eq!(body.len(), plaintext.len());
        assert_eq!(&body, &plaintext);
    }

    #[tokio::test]
    async fn prepare_stream_416_out_of_range() {
        let plaintext = b"fLaC test data".to_vec();
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");

        // 请求超出范围的区间（加密文件包含 footer，总长 > plaintext）
        let (code, _body) =
            fetch_via_proxy(&prepared.uri, plaintext.len() as u64, Some(99999)).await;
        assert_eq!(code, 416);
    }

    #[tokio::test]
    async fn prepare_stream_caps_at_audio_len() {
        let plaintext = b"fLaC caps".to_vec();
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");

        // bytes=0- 返回长度应 == audio_len（plaintext 长度），无 footer 字节
        let (code, body) = fetch_via_proxy(&prepared.uri, 0, None).await;
        assert_eq!(code, 206, "Range 请求一律返回 206");
        assert_eq!(body.len(), plaintext.len());
        assert_eq!(&body, &plaintext);
    }

    #[tokio::test]
    async fn prepare_stream_falls_back_without_cdn_range() {
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

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 回退应成功");

        // 应返回 file:// URI
        assert!(
            prepared.uri.starts_with("file://"),
            "expected file:// URI, got {}",
            prepared.uri
        );

        // 验证内容一致
        let path = prepared.uri.strip_prefix("file://").unwrap();
        let decoded = std::fs::read(path).unwrap();
        assert_eq!(decoded, plaintext, "回退内容应与明文一致");
    }

    #[tokio::test]
    async fn prepare_stream_guard_drop_stops_server() {
        let plaintext = b"fLaC guard test".to_vec();
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");

        // 提取端口
        let uri = prepared.uri.clone();
        assert!(uri.starts_with("http://127.0.0.1:"));
        let port_str = uri.strip_prefix("http://127.0.0.1:").unwrap();
        let port_str = port_str.strip_suffix("/stream").unwrap_or(port_str);
        let port: u16 = port_str.parse().unwrap();

        // drop prepared → guard drop 发送停止信号
        drop(prepared);

        // 给服务器一点时间关闭
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // 端口应不再接受连接
        let result = TcpStream::connect(format!("127.0.0.1:{port}")).await;
        assert!(result.is_err(), "drop PreparedMedia 后端口应被释放");
    }

    #[tokio::test]
    async fn prepare_stream_keeps_alive() {
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..2048).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");

        // 同一个 reqwest Client 连发两个 Range 请求
        let client = reqwest::Client::new();
        let resp1 = client
            .get(format!("{}/stream", prepared.uri))
            .header("Range", "bytes=0-99")
            .send()
            .await
            .expect("请求1失败");
        assert_eq!(resp1.status().as_u16(), 206);
        let body1 = resp1.bytes().await.unwrap();
        assert_eq!(&body1[..], &plaintext[0..100]);

        let resp2 = client
            .get(format!("{}/stream", prepared.uri))
            .header("Range", "bytes=100-199")
            .send()
            .await
            .expect("请求2失败");
        assert_eq!(resp2.status().as_u16(), 206);
        let body2 = resp2.bytes().await.unwrap();
        assert_eq!(&body2[..], &plaintext[100..200]);
    }

    #[tokio::test]
    async fn prepare_stream_embedded_ekey_from_tail() {
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

        // 使用 prepare_stream（无 API ekey → 从 QTag 尾部提取）
        let prepared = prepare_stream(&server.uri(), None, None)
            .await
            .expect("prepare_stream 应成功（无 API ekey）");

        assert!(prepared.uri.starts_with("http://127.0.0.1:"));

        // 全量请求验证内容一致
        let (code, body) = fetch_via_proxy(&prepared.uri, 0, None).await;
        assert_eq!(code, 206, "Range 请求一律返回 206");
        assert_eq!(&body, plaintext);
    }

    #[tokio::test]
    async fn prepare_stream_seek_back_after_forward() {
        let plaintext = {
            let mut v = b"fLaC".to_vec();
            v.extend((0..8192).map(|i| (i % 256) as u8));
            v
        };
        let key = b"0123456789abcdefghij";
        let (server, ekey) = setup_range_cdn(&plaintext, key, true).await;

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");

        // 先 forward seek
        let (code1, body1) = fetch_via_proxy(&prepared.uri, 5000, Some(6000)).await;
        assert_eq!(code1, 206);
        assert_eq!(&body1, &plaintext[5000..=6000]);

        // 再 backward seek（无状态污染）
        let (code2, body2) = fetch_via_proxy(&prepared.uri, 0, Some(1000)).await;
        assert_eq!(code2, 206);
        assert_eq!(&body2, &plaintext[0..=1000]);
    }

    #[tokio::test]
    async fn prepare_stream_returns_502_for_invalid_runtime_206() {
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
                if range == "bytes=0-3" {
                    return ResponseTemplate::new(206)
                        .insert_header("Content-Range", format!("bytes 1-1/{total_len}"))
                        .set_body_bytes(encrypted[0..1].to_vec());
                }
                if let Some((start, end)) = parse_range_value(range) {
                    let end = end.min(total_len - 1);
                    return ResponseTemplate::new(206)
                        .insert_header("Content-Range", format!("bytes {start}-{end}/{total_len}"))
                        .set_body_bytes(encrypted[start as usize..=end as usize].to_vec());
                }
                ResponseTemplate::new(416)
            })
            .mount(&server)
            .await;

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");
        let (code, body) = fetch_via_proxy(&prepared.uri, 0, Some(3)).await;
        assert_eq!(code, 502);
        assert_eq!(body, b"Bad Gateway");
    }

    #[tokio::test]
    async fn prepare_stream_returns_502_for_short_runtime_206_body() {
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
                    let end = end.min(total_len - 1);
                    let body_end = if range == "bytes=0-3" {
                        0
                    } else {
                        end as usize
                    };
                    return ResponseTemplate::new(206)
                        .insert_header("Content-Range", format!("bytes {start}-{end}/{total_len}"))
                        .set_body_bytes(encrypted[start as usize..=body_end].to_vec());
                }
                ResponseTemplate::new(416)
            })
            .mount(&server)
            .await;

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");
        let (code, body) = fetch_via_proxy(&prepared.uri, 0, Some(3)).await;
        assert_eq!(code, 502);
        assert_eq!(body, b"Bad Gateway");
    }

    #[tokio::test]
    async fn prepare_stream_200_defense_short_body_returns_502() {
        // 200 防御路径：CDN body 远短于请求的 start，首个分块即为错误，
        // stream_range_body 在写入响应头前拉取分块，故返回干净的 502。
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

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");

        // bytes=100-199：start=100 > short body 50 字节 → 全部被 skip，流提前结束
        let (code, body) = fetch_via_proxy(&prepared.uri, 100, Some(199)).await;
        assert_eq!(code, 502);
        assert_eq!(body, b"Bad Gateway");
    }

    #[tokio::test]
    async fn prepare_stream_200_defense_path() {
        // 测试 200 防御路径：CDN probe 返回 206（bytes=0-0 成功），但
        // 后续数据区间请求返回 200 全量 body。open 必须流式跳过 start 之前的
        // 字节并正确解密 [start..=end]。
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

        // HEAD mock
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", total_len.to_string())
                    .insert_header("Accept-Ranges", "bytes"),
            )
            .mount(&server)
            .await;

        // 组合 mock：bytes=0-0 → 206；尾部区间 → 206；数据区间 → 200
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
                    // 尾部区间返回 206（prepare_stream 需要检测 footer）
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

        let prepared = prepare_stream(&server.uri(), Some(&ekey), None)
            .await
            .expect("prepare_stream 应成功");

        // 请求 bytes=100-199 → 应通过 200 跳过路径正确解密
        let (code, body) = fetch_via_proxy(&prepared.uri, 100, Some(199)).await;
        assert_eq!(code, 206);
        assert_eq!(&body, &plaintext[100..=199]);
    }
}
