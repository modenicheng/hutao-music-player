//! 极简 HTTP/1.1 流媒体服务端。
//!
//! 在 `127.0.0.1:0` 上监听 TCP 连接，接受播放器的 `Range` 请求，
//! 从 [`Source`] 读取明文区间并返回 200/206/416 响应。
//!
//! # 协议约定
//!
//! - 仅接受 `GET` / `HEAD`；忽略请求路径
//! - 默认 HTTP/1.1 keep-alive；`Connection: close` 时单请求后关闭
//! - 所有响应带 `Accept-Ranges: bytes`、`Content-Type: application/octet-stream`

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tracing::debug;

use super::range::{ByteRange, clamp_end, parse_range};

/// [`Source::read_range`] 返回的 future 类型别名，避免 [`Arc<dyn Source>`] 中的
/// 复杂类型标注。
type ReadRangeFuture<'a> =
    Pin<Box<dyn Future<Output = std::io::Result<Cow<'a, [u8]>>> + Send + 'a>>;

/// 可随机访问的音频数据源。
///
/// Task 1 测试使用假实现（返回静态借用数据）；
/// Task 2 实现 CDN 拉取 + QMC2 解密管道。
pub trait Source: Send + Sync {
    /// 音频总字节数。
    fn audio_len(&self) -> u64;

    /// 读取 `range` 指定区间的明文字节。
    ///
    /// 返回 [`Cow`] 以允许测试返回借用数据而无需复制；
    /// Task 2 的实际实现将返回 `Cow::Owned`。
    fn read_range<'a>(&'a self, range: ByteRange) -> ReadRangeFuture<'a>;
}

/// 启动 HTTP 代理 accept 循环。
///
/// - `listener`：已绑定的 TCP 监听器（建议 `127.0.0.1:0`）
/// - `source`：音频数据源（共享引用）
/// - `stop`：收到信号后退出 accept 循环；已接受的连接继续运行至结束
pub async fn serve(
    listener: TcpListener,
    source: Arc<dyn Source>,
    mut stop: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut stop => {
                debug!("收到关闭信号，停止 accept 循环");
                return;
            }
            result = listener.accept() => {
                match result {
                    Ok((stream, addr)) => {
                        debug!(%addr, "新连接");
                        let source = Arc::clone(&source);
                        tokio::spawn(async move {
                            handle_connection(stream, source).await;
                        });
                    }
                    Err(e) => {
                        debug!(%e, "accept 错误，退出循环");
                        break;
                    }
                }
            }
        }
    }
}

/// 处理单个 TCP 连接：循环读取 HTTP 请求并响应。
async fn handle_connection(stream: TcpStream, source: Arc<dyn Source>) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line_buf = String::new();

    let audio_len = source.audio_len();

    loop {
        line_buf.clear();

        // ── 读取请求行 ──────────────────────────────────────────
        match reader.read_line(&mut line_buf).await {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                debug!(%e, "读取请求行失败");
                break;
            }
        }

        let request_line = trim_crlf(&line_buf);
        if request_line.is_empty() {
            continue; // 跳过空行
        }

        // 解析 "METHOD SP path SP HTTP/1.1"
        let mut parts = request_line.splitn(3, ' ');
        let method = parts.next().unwrap_or("");
        let _path = parts.next().unwrap_or("/");
        let _version = parts.next().unwrap_or("HTTP/1.1");

        // 仅接受 GET / HEAD
        if method != "GET" && method != "HEAD" {
            write_quick_response(&mut writer, 405, true).await;
            break;
        }

        let is_head = method == "HEAD";

        // ── 读取请求头 ──────────────────────────────────────────
        let mut connection_close = false;
        let mut range_value: Option<String> = None;

        loop {
            line_buf.clear();
            match reader.read_line(&mut line_buf).await {
                Ok(0) => break, // EOF（无头部直接结束）
                Ok(_) => {}
                Err(e) => {
                    debug!(%e, "读取请求头失败");
                    break;
                }
            }

            let header_line = trim_crlf(&line_buf);
            if header_line.is_empty() {
                break; // 头部结束
            }

            // 解析 "Key: Value"
            if let Some((key, value)) = header_line.split_once(": ") {
                if key.eq_ignore_ascii_case("Connection") {
                    connection_close = value.eq_ignore_ascii_case("close");
                } else if key.eq_ignore_ascii_case("Range") {
                    range_value = Some(value.to_string());
                }
            }
        }

        // ── 处理请求 ────────────────────────────────────────────
        if let Some(ref range_str) = range_value {
            match parse_range(range_str, audio_len) {
                Ok(br) => {
                    let clamped = clamp_end(br.start, br.end, audio_len);
                    if clamped.start >= audio_len {
                        // 416
                        let cr = format!("bytes */{}", audio_len);
                        write_response(&mut writer, 416, &[], Some(&cr), connection_close, false)
                            .await;
                    } else {
                        match source.read_range(clamped).await {
                            Ok(data) => {
                                let cr = format!(
                                    "bytes {}-{}/{}",
                                    clamped.start, clamped.end, audio_len
                                );
                                write_response(
                                    &mut writer,
                                    206,
                                    &data,
                                    Some(&cr),
                                    connection_close,
                                    is_head,
                                )
                                .await;
                            }
                            Err(e) => {
                                debug!(%e, "读取音频区间失败，关闭连接");
                                break;
                            }
                        }
                    }
                }
                Err(_) => {
                    // Malformed / Unsatisfiable → 416
                    let cr = format!("bytes */{}", audio_len);
                    write_response(&mut writer, 416, &[], Some(&cr), connection_close, false).await;
                }
            }
        } else {
            // 无 Range → 200，返回全部内容
            let full = ByteRange {
                start: 0,
                end: audio_len.saturating_sub(1),
            };
            match source.read_range(full).await {
                Ok(data) => {
                    write_response(&mut writer, 200, &data, None, connection_close, is_head).await;
                }
                Err(e) => {
                    debug!(%e, "读取全量音频失败，关闭连接");
                    break;
                }
            }
        }

        // ── 连接生命周期 ────────────────────────────────────────
        if connection_close || is_head {
            // HEAD 只服务一次请求；Connection: close 显式关闭
            break;
        }
    }
}

/// 写入 HTTP 响应（带可选 body）。
async fn write_response(
    writer: &mut (impl AsyncWriteExt + Unpin),
    code: u16,
    body: &[u8],
    content_range: Option<&str>,
    connection_close: bool,
    head_only: bool,
) {
    let mut resp = format!("HTTP/1.1 {}\r\n", status_line(code));
    resp.push_str("Accept-Ranges: bytes\r\n");
    resp.push_str("Content-Type: application/octet-stream\r\n");

    if let Some(cr) = content_range {
        resp.push_str(&format!("Content-Range: {}\r\n", cr));
    }

    resp.push_str(&format!("Content-Length: {}\r\n", body.len()));

    if connection_close {
        resp.push_str("Connection: close\r\n");
    }

    resp.push_str("\r\n");

    // 写入状态行 + 头部
    if writer.write_all(resp.as_bytes()).await.is_err() {
        return;
    }

    // HEAD 请求不发送 body
    if !head_only && !body.is_empty() && writer.write_all(body).await.is_err() {
        return;
    }

    let _ = writer.flush().await;
}

/// 快速写入简单响应（无 body，用于 405 等错误）。
async fn write_quick_response(
    writer: &mut (impl AsyncWriteExt + Unpin),
    code: u16,
    connection_close: bool,
) {
    let mut resp = format!("HTTP/1.1 {}\r\n", status_line(code));
    resp.push_str("Accept-Ranges: bytes\r\n");
    resp.push_str("Content-Type: application/octet-stream\r\n");
    resp.push_str("Content-Length: 0\r\n");

    if connection_close {
        resp.push_str("Connection: close\r\n");
    }

    resp.push_str("\r\n");

    let _ = writer.write_all(resp.as_bytes()).await;
    let _ = writer.flush().await;
}

/// 返回 HTTP 状态码对应的 reason phrase。
///
/// 供测试断言及 `write_response` 内部使用。
pub fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        206 => "206 Partial Content",
        400 => "400 Bad Request",
        405 => "405 Method Not Allowed",
        416 => "416 Range Not Satisfiable",
        _ => "500 Internal Server Error",
    }
}

/// 去除 `\r\n` 或 `\n` 尾部的辅助函数。
fn trim_crlf(s: &str) -> &str {
    s.trim_end_matches(['\r', '\n'])
}

// ── 测试 ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    // ── 测试辅助：假 Source ──────────────────────────────────────

    /// 假音频源：返回固定长度 `u8` 序列 `[0, 1, 2, ..., audio_len-1]`。
    struct FakeSource {
        data: Vec<u8>,
    }

    impl FakeSource {
        fn new(audio_len: u64) -> Self {
            let data: Vec<u8> = (0u8..).take(audio_len as usize).collect();
            Self { data }
        }
    }

    impl Source for FakeSource {
        fn audio_len(&self) -> u64 {
            self.data.len() as u64
        }

        fn read_range<'a>(
            &'a self,
            range: ByteRange,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<Cow<'a, [u8]>>> + Send + 'a>> {
            Box::pin(async move {
                let start = range.start as usize;
                let end = (range.end as usize).min(self.data.len().saturating_sub(1));
                Ok(Cow::Borrowed(&self.data[start..=end]))
            })
        }
    }

    // ── 测试辅助：HTTP 响应解析器 ────────────────────────────────

    /// 从 TCP 流读取一次完整 HTTP 响应，返回 `(状态码, 头部表, 正文)`。
    ///
    /// 内部使用 `Content-Length` 判断正文长度；读取完毕后连接仍保持
    /// 打开，可用于 keep-alive 测试。
    async fn read_response(
        stream: &mut tokio::net::TcpStream,
    ) -> (u16, HashMap<String, String>, Vec<u8>) {
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];

        // 读取直到出现 \r\n\r\n
        loop {
            let n = stream.read(&mut buf).await.expect("读取响应失败");
            if n == 0 {
                panic!("连接在读取头部前关闭");
            }
            raw.extend_from_slice(&buf[..n]);
            if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        // 找到 \r\n\r\n 的位置
        let header_end = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("未找到头部结束标记")
            + 4;

        let header_bytes = &raw[..header_end - 2]; // 去掉尾部 \r\n
        let header_text = std::str::from_utf8(header_bytes).expect("头部非 UTF-8");

        // 解析状态行
        let mut lines = header_text.split("\r\n");
        let status_line = lines.next().expect("缺少状态行");
        let parts: Vec<&str> = status_line.splitn(3, ' ').collect();
        assert_eq!(parts[0], "HTTP/1.1", "响应应为 HTTP/1.1");
        let code: u16 = parts[1].parse().expect("状态码非数字");

        // 解析头部
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(": ") {
                headers.insert(k.to_lowercase(), v.to_string());
            }
        }

        // 读取正文
        let content_length: usize = headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        // 先取已读取的 body 部分
        let mut body = raw[header_end..].to_vec();

        while body.len() < content_length {
            let remaining = content_length - body.len();
            let limit = remaining.min(buf.len());
            let n = stream.read(&mut buf[..limit]).await.expect("读取正文失败");
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }

        (code, headers, body)
    }

    /// 启动服务端并返回 `(local_addr, stop_tx)`。`stop_tx` drop 后 accept
    /// 循环退出。
    async fn spawn_server(source: Arc<dyn Source>) -> (std::net::SocketAddr, oneshot::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
        let addr = listener.local_addr().expect("获取地址失败");
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let source_clone = Arc::clone(&source);
        tokio::spawn(async move {
            serve(listener, source_clone, stop_rx).await;
        });
        (addr, stop_tx)
    }

    // ── 测试用例 ─────────────────────────────────────────────────

    #[tokio::test]
    async fn serve_serves_full_body_without_range() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source) as Arc<dyn Source>).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let (code, headers, body) = read_response(&mut stream).await;

        assert_eq!(code, 200);
        assert_eq!(
            headers.get("content-length").map(|v| v.as_str()),
            Some("100")
        );
        assert_eq!(
            headers.get("accept-ranges").map(|v| v.as_str()),
            Some("bytes")
        );
        assert_eq!(body.len(), 100);
        assert_eq!(&body, &source.data);
    }

    #[tokio::test]
    async fn serve_serves_range_206() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source) as Arc<dyn Source>).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nRange: bytes=5-9\r\n\r\n")
            .await
            .unwrap();

        let (code, headers, body) = read_response(&mut stream).await;

        assert_eq!(code, 206);
        assert_eq!(
            headers.get("content-range").map(|v| v.as_str()),
            Some("bytes 5-9/100")
        );
        assert_eq!(headers.get("content-length").map(|v| v.as_str()), Some("5"));
        assert_eq!(&body, &[5, 6, 7, 8, 9]);
    }

    #[tokio::test]
    async fn serve_416_beyond_audio() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source) as Arc<dyn Source>).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nRange: bytes=100-\r\n\r\n")
            .await
            .unwrap();

        let (code, headers, body) = read_response(&mut stream).await;

        assert_eq!(code, 416);
        assert_eq!(
            headers.get("content-range").map(|v| v.as_str()),
            Some("bytes */100")
        );
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn serve_caps_range_at_audio_len() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source) as Arc<dyn Source>).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nRange: bytes=95-\r\n\r\n")
            .await
            .unwrap();

        let (code, headers, body) = read_response(&mut stream).await;

        assert_eq!(code, 206);
        assert_eq!(
            headers.get("content-range").map(|v| v.as_str()),
            Some("bytes 95-99/100")
        );
        assert_eq!(body.len(), 5);
        assert_eq!(&body, &[95, 96, 97, 98, 99]);
    }

    #[tokio::test]
    async fn serve_head_no_body() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source) as Arc<dyn Source>).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"HEAD / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let (code, headers, body) = read_response(&mut stream).await;

        assert_eq!(code, 200);
        assert_eq!(
            headers.get("content-length").map(|v| v.as_str()),
            Some("100")
        );
        assert!(body.is_empty(), "HEAD 不应携带正文");
    }

    #[tokio::test]
    async fn serve_keepalive_two_requests() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source) as Arc<dyn Source>).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");

        // 第一个请求：无 Range
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let (code1, _headers1, body1) = read_response(&mut stream).await;
        assert_eq!(code1, 200);
        assert_eq!(body1.len(), 100);

        // 第二个请求：带 Range（同一连接）
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nRange: bytes=10-19\r\n\r\n")
            .await
            .unwrap();
        let (code2, headers2, body2) = read_response(&mut stream).await;
        assert_eq!(code2, 206);
        assert_eq!(
            headers2.get("content-range").map(|v| v.as_str()),
            Some("bytes 10-19/100")
        );
        assert_eq!(&body2, &[10, 11, 12, 13, 14, 15, 16, 17, 18, 19]);
    }

    #[tokio::test]
    async fn serve_rejects_unsupported_method() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source) as Arc<dyn Source>).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello")
            .await
            .unwrap();

        let (code, _headers, _body) = read_response(&mut stream).await;

        assert_eq!(code, 405);
    }

    #[tokio::test]
    async fn serve_stops_on_shutdown() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, stop_tx) = spawn_server(Arc::clone(&source) as Arc<dyn Source>).await;

        // 确认服务端在运行：发一个请求应成功
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let (code, _headers, _body) = read_response(&mut stream).await;
        assert_eq!(code, 200);

        // 发送关闭信号
        drop(stop_tx);

        // 等待一小段时间让 accept 循环退出
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // 新连接应该被拒绝（listener 已被 drop / accept 循环已退出）
        // 注意：由于 serve 里 listener 是 move 进去的，drop stop_tx 后
        // accept 循环退出，listener 随之 drop → 端口释放
        let result = tokio::net::TcpStream::connect(addr).await;
        // 端口可能已被释放，连接应失败
        assert!(result.is_err(), "停止后新连接应被拒绝");
    }

    // ── status_line 单元测试 ─────────────────────────────────────

    #[test]
    fn status_line_known_codes() {
        assert_eq!(status_line(200), "200 OK");
        assert_eq!(status_line(206), "206 Partial Content");
        assert_eq!(status_line(400), "400 Bad Request");
        assert_eq!(status_line(405), "405 Method Not Allowed");
        assert_eq!(status_line(416), "416 Range Not Satisfiable");
    }
}
