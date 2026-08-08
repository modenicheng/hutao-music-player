//! CDN 流式数据源：探测 CDN Range 支持 → 解析 QMC2 尾部 →
//! 构建流密码 → 按需拉取并解密区间。
//!
//! 若 CDN 不支持 `Range`（返回 200 或无 `Content-Range`），
//! 自动回退到 `decrypt` 全量下载-解密-缓存流程。

use std::io;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::StreamExt;
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
    _total_len: u64,
    /// 并发限制（最多 4 个并行 range 请求）。
    sem: Arc<Semaphore>,
}

impl Source for StreamSource {
    fn audio_len(&self) -> u64 {
        self.audio_len
    }

    fn read_range<'a>(
        &'a self,
        range: ByteRange,
    ) -> Pin<
        Box<dyn std::future::Future<Output = io::Result<std::borrow::Cow<'a, [u8]>>> + Send + 'a>,
    > {
        Box::pin(async move {
            let _permit = self
                .sem
                .acquire()
                .await
                .map_err(|e| io::Error::other(format!("信号量获取失败: {e}")))?;

            let start = range.start;
            let end = range.end;
            let range_header = format!("bytes={start}-{end}");

            debug!(cdn_url = %self.cdn_url, %range_header, "请求 CDN 区间");

            let response = self
                .client
                .get(&self.cdn_url)
                .header("Range", &range_header)
                .send()
                .await
                .map_err(|e| io::Error::other(format!("CDN 请求失败: {e}")))?;

            let status = response.status();

            let body_bytes: Vec<u8> = if status == reqwest::StatusCode::PARTIAL_CONTENT {
                // 206 — 预期路径，读取全部 body
                response
                    .bytes()
                    .await
                    .map_err(|e| io::Error::other(format!("读取 206 body 失败: {e}")))?
                    .to_vec()
            } else if status == reqwest::StatusCode::OK {
                // 200 — CDN 忽略了 Range，读取全量后切片（防御性路径）
                warn!(cdn_url = %self.cdn_url, "CDN 返回 200（忽略 Range），正在读取全量");
                let full_bytes = read_full_body(response).await?;
                let slice_start = start as usize;
                let slice_end = (end as usize).min(full_bytes.len().saturating_sub(1));
                if slice_start >= full_bytes.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "区间超出全量范围",
                    ));
                }
                full_bytes[slice_start..=slice_end].to_vec()
            } else {
                return Err(io::Error::other(format!("CDN 返回非预期状态码: {status}")));
            };

            // 解密
            let mut buf = body_bytes.to_vec();
            let decrypt_len = buf.len();
            // 分块解密，避免一次性处理可能超大的 buf
            const CHUNK: usize = 256 * 1024; // 256 KiB
            let mut dec_offset = start as usize;
            for chunk in buf.chunks_mut(CHUNK) {
                self.cipher.decrypt(dec_offset, chunk);
                dec_offset += chunk.len();
            }

            debug!(start, end, len = decrypt_len, "CDN 区间解密完成");

            Ok(std::borrow::Cow::Owned(buf))
        })
    }
}

/// 流式读取全量 body（200 回退路径），按 256 KiB 分块累积。
async fn read_full_body(response: reqwest::Response) -> io::Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut acc: Vec<u8> = Vec::new();
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| io::Error::other(format!("读取流错误: {e}")))?;
        acc.extend_from_slice(&chunk);
    }
    Ok(acc)
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
    }

    let audio_len: usize = match &footer {
        Some(Footer::QTag { audio_len: al } | Footer::V1 { audio_len: al }) => *al,
        None => total_len as usize,
    };

    // 3. 获取密钥
    let cipher: Arc<dyn Qmc2Cipher> = if let Some(e) = ekey {
        let c = qmc2::decrypt_factory(e).map_err(MediaError::Key)?;
        Arc::from(c)
    } else {
        // 无 API ekey → 从尾部提取内嵌 ekey
        // 总是从 audio_len 拉到末尾获取完整尾部，供 embedded_ekey_from_bytes 使用
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
        _total_len: total_len,
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
/// 先发 HEAD 拿 Content-Length；再发 `GET Range: bytes=0-0` 确认：
/// - 返回 206 且有有效的 `Content-Range: bytes 0-0/{total}` 且 total>0
/// - 不满足 → 报错（触发回退）
async fn probe_cdn(client: &reqwest::Client, url: &str) -> Result<u64, MediaError> {
    // HEAD
    let head_resp = client
        .head(url)
        .send()
        .await
        .map_err(|e| MediaError::Network(format!("HEAD 请求失败: {e}")))?;

    let head_status = head_resp.status();
    if !head_status.is_success() {
        return Err(MediaError::HttpStatus(head_status.as_u16()));
    }

    let _content_length = head_resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

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
        .and_then(parse_content_range_total)
        .ok_or_else(|| MediaError::Unsupported("CDN 未返回有效的 Content-Range".to_string()))?;

    if total == 0 {
        return Err(MediaError::Unsupported("CDN 报告零长度".to_string()));
    }

    debug!(total, "CDN Range 探测成功");
    Ok(total)
}

/// 解析 `Content-Range` 头值中的 total 字段。
///
/// 期望格式 `bytes s-e/total`，返回 `total`。
fn parse_content_range_total(header: &str) -> Option<u64> {
    // 剥离 "bytes " 前缀
    let spec = header.strip_prefix("bytes ")?;
    // 提取 "/total" 部分
    let (_range, total_str) = spec.rsplit_once('/')?;
    total_str.parse::<u64>().ok()
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

        // 请求 bytes=5000-6000（seek 行为）
        // 注：plaintext 仅 4100 字节，5000 超出 audio_len，代理返回 416
        // 改用有效区间
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
}
