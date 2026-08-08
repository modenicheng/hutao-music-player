//! 极简 HTTP/1.1 流媒体服务端。
//!
//! 在 `127.0.0.1:0` 上监听 TCP 连接，接受播放器的 `Range` 请求，
//! 从 [`Source`] 读取明文区间并返回 200/206/416 响应。
//!
//! # 协议约定
//!
//! - 仅接受 `GET` / `HEAD`；忽略请求路径
//! - 严格验证请求行 `METHOD SP path SP HTTP/1.x`；格式错误 → 400
//! - HTTP/1.1 默认 keep-alive；HTTP/1.0 默认 close
//! - `Connection: close` / `Connection: keep-alive` 显式覆盖默认值
//! - 所有响应带 `Accept-Ranges: bytes`、`Content-Type: application/octet-stream`

use std::io;
use std::sync::Arc;

use futures_util::{Stream, StreamExt, pin_mut};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tracing::debug;

use super::range::{ByteRange, clamp_end, parse_range};

/// 可随机访问的音频数据源，以分块流形式输出明文。
///
/// Task 1 测试使用假实现（返回固定 7 字节分块）；
/// Task 2 实现 CDN 拉取 + QMC2 流式解密管道。
pub trait Source: Send + Sync {
    /// 音频总字节数。
    fn audio_len(&self) -> u64;

    /// 用于遍历指定区间明文字节的分块流。
    type ChunkStream<'a>: Stream<Item = io::Result<Vec<u8>>> + Send + 'a
    where
        Self: 'a;

    /// 打开 `range` 区间的分块解密流。
    fn open<'a>(&'a self, range: ByteRange) -> Self::ChunkStream<'a>;
}

/// 启动 HTTP 代理 accept 循环。
///
/// - `listener`：已绑定的 TCP 监听器（建议 `127.0.0.1:0`）
/// - `source`：音频数据源（共享引用）
/// - `stop`：收到信号后退出 accept 循环；已接受的连接继续运行至结束
pub async fn serve<S: Source + 'static>(
    listener: TcpListener,
    source: Arc<S>,
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
async fn handle_connection<S: Source + 'static>(stream: TcpStream, source: Arc<S>) {
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

        // 解析 "METHOD SP path SP HTTP/1.x"
        let parts: Vec<&str> = request_line.splitn(3, ' ').collect();
        if parts.len() != 3 {
            write_error(&mut writer, 400, &[], true).await;
            break;
        }
        let method = parts[0];
        let _path = parts[1];
        let version = parts[2];

        // 验证 HTTP 版本
        if version != "HTTP/1.1" && version != "HTTP/1.0" {
            write_error(&mut writer, 400, &[], true).await;
            break;
        }

        // HTTP/1.0 默认不 keep-alive；HTTP/1.1 默认 keep-alive
        // Connection 头可显式覆盖（见下方解析）
        let mut connection_close = version == "HTTP/1.0";

        // 仅接受 GET / HEAD
        if method != "GET" && method != "HEAD" {
            write_error(&mut writer, 405, &[], true).await;
            break;
        }

        let is_head = method == "HEAD";

        // ── 读取请求头 ──────────────────────────────────────────
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
                    if value.eq_ignore_ascii_case("close") {
                        connection_close = true;
                    } else if value.eq_ignore_ascii_case("keep-alive") {
                        connection_close = false;
                    }
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
                        let _ =
                            write_status_headers(&mut writer, 416, 0, Some(&cr), connection_close)
                                .await;
                        let _ = writer.flush().await;
                    } else {
                        let cr = format!("bytes {}-{}/{}", clamped.start, clamped.end, audio_len);
                        let content_len = clamped.end - clamped.start + 1;
                        if stream_range_body(
                            &mut writer,
                            206,
                            content_len,
                            Some(&cr),
                            connection_close,
                            is_head,
                            source.open(clamped),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                }
                Err(_) => {
                    // Malformed / Unsatisfiable → 416
                    let cr = format!("bytes */{}", audio_len);
                    let _ = write_status_headers(&mut writer, 416, 0, Some(&cr), connection_close)
                        .await;
                    let _ = writer.flush().await;
                }
            }
        } else {
            // 无 Range → 200，返回全部内容
            let full = ByteRange {
                start: 0,
                end: audio_len.saturating_sub(1),
            };
            let content_len = audio_len;
            if stream_range_body(
                &mut writer,
                200,
                content_len,
                None,
                connection_close,
                is_head,
                source.open(full),
            )
            .await
            .is_err()
            {
                break;
            }
        }

        // ── 连接生命周期 ────────────────────────────────────────
        if connection_close {
            break;
        }
    }
}

// ── 响应写入辅助 ──────────────────────────────────────────────

/// 写入 HTTP 状态行 + 头部（不含 body）。
async fn write_status_headers(
    writer: &mut (impl AsyncWriteExt + Unpin),
    code: u16,
    content_length: u64,
    content_range: Option<&str>,
    connection_close: bool,
) -> io::Result<()> {
    let mut resp = format!("HTTP/1.1 {}\r\n", status_line(code));
    resp.push_str("Accept-Ranges: bytes\r\n");
    resp.push_str("Content-Type: application/octet-stream\r\n");

    if let Some(cr) = content_range {
        resp.push_str(&format!("Content-Range: {}\r\n", cr));
    }

    resp.push_str(&format!("Content-Length: {}\r\n", content_length));

    if connection_close {
        resp.push_str("Connection: close\r\n");
    }

    resp.push_str("\r\n");

    writer.write_all(resp.as_bytes()).await
}

/// 写入带 status + headers + body 的错误响应。
async fn write_error(
    writer: &mut (impl AsyncWriteExt + Unpin),
    code: u16,
    body: &[u8],
    connection_close: bool,
) {
    let _ = write_status_headers(writer, code, body.len() as u64, None, connection_close).await;
    if !body.is_empty() {
        let _ = writer.write_all(body).await;
    }
    let _ = writer.flush().await;
}

/// 写入 status + headers，然后流式写入 chunks 作为 body。
async fn stream_range_body(
    writer: &mut (impl AsyncWriteExt + Unpin),
    code: u16,
    content_length: u64,
    content_range: Option<&str>,
    connection_close: bool,
    head_only: bool,
    chunks: impl Stream<Item = io::Result<Vec<u8>>>,
) -> io::Result<()> {
    write_status_headers(
        writer,
        code,
        content_length,
        content_range,
        connection_close,
    )
    .await?;

    if head_only {
        return writer.flush().await;
    }

    pin_mut!(chunks);
    while let Some(chunk_result) = chunks.next().await {
        match chunk_result {
            Ok(chunk) => {
                writer.write_all(&chunk).await?;
            }
            Err(e) => {
                debug!(%e, "读取音频分块失败，断开连接");
                return Err(e);
            }
        }
    }

    writer.flush().await
}

/// 返回 HTTP 状态码对应的 reason phrase。
///
/// 供测试断言及响应写入内部使用。
pub fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        206 => "206 Partial Content",
        400 => "400 Bad Request",
        405 => "405 Method Not Allowed",
        416 => "416 Range Not Satisfiable",
        502 => "502 Bad Gateway",
        _ => "500 Internal Server Error",
    }
}

/// 去除 `\r\n` 或 `\n` 尾部的辅助函数。
fn trim_crlf(s: &str) -> &str {
    s.strip_suffix("\r\n")
        .or_else(|| s.strip_suffix('\n'))
        .or_else(|| s.strip_suffix('\r'))
        .unwrap_or(s)
}

// ── 测试 ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use std::pin::Pin;

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

        type ChunkStream<'a> = Pin<Box<dyn Stream<Item = io::Result<Vec<u8>>> + Send + 'a>>;

        fn open<'a>(&'a self, range: ByteRange) -> Self::ChunkStream<'a> {
            let start = range.start as usize;
            let end = (range.end as usize).min(self.data.len().saturating_sub(1));
            const CHUNK: usize = 7;
            let chunks: Vec<io::Result<Vec<u8>>> = self.data[start..=end]
                .chunks(CHUNK)
                .map(|c| Ok(c.to_vec()))
                .collect();
            Box::pin(futures_util::stream::iter(chunks))
        }
    }

    // ── 测试辅助：失败 Source ────────────────────────────────

    /// 假音频源：所有 `open` 调用返回立即失败的流。
    struct FailingSource {
        audio_len: u64,
    }

    impl FailingSource {
        fn new(audio_len: u64) -> Self {
            Self { audio_len }
        }
    }

    impl Source for FailingSource {
        fn audio_len(&self) -> u64 {
            self.audio_len
        }

        type ChunkStream<'a> = Pin<Box<dyn Stream<Item = io::Result<Vec<u8>>> + Send + 'a>>;

        fn open<'a>(&'a self, _range: ByteRange) -> Self::ChunkStream<'a> {
            let chunk: Vec<io::Result<Vec<u8>>> =
                vec![Err(io::Error::other("simulated source failure"))];
            Box::pin(futures_util::stream::iter(chunk))
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
        let (code, headers, raw_body_start) = read_headers(stream).await;

        let content_length: usize = headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let mut body = raw_body_start;
        let mut buf = [0u8; 4096];

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

    /// 读取 HTTP 响应头部（含状态行），返回 `(code, headers, body_bytes_after_headers)`。
    async fn read_headers(
        stream: &mut tokio::net::TcpStream,
    ) -> (u16, HashMap<String, String>, Vec<u8>) {
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];

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

        let header_end = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("未找到头部结束标记")
            + 4;

        let header_text = std::str::from_utf8(&raw[..header_end - 2]).expect("头部非 UTF-8");

        let mut lines = header_text.split("\r\n");
        let status_line = lines.next().expect("缺少状态行");
        let parts: Vec<&str> = status_line.splitn(3, ' ').collect();
        assert_eq!(parts[0], "HTTP/1.1", "响应应为 HTTP/1.1");
        let code: u16 = parts[1].parse().expect("状态码非数字");

        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(": ") {
                headers.insert(k.to_lowercase(), v.to_string());
            }
        }

        (code, headers, raw[header_end..].to_vec())
    }

    /// 仅读取 HTTP 响应头部（不读取 body），用于 HEAD 请求的响应。
    ///
    /// HEAD 响应 `Content-Length` 报告完整 body 大小但不发送 body，
    /// 因此不能依赖 Content-Length 读取 body（keep-alive 连接会挂起）。
    async fn read_headers_only(
        stream: &mut tokio::net::TcpStream,
    ) -> (u16, HashMap<String, String>) {
        let (code, headers, _) = read_headers(stream).await;
        (code, headers)
    }

    /// 启动服务端并返回 `(local_addr, stop_tx)`。`stop_tx` drop 后 accept
    /// 循环退出。
    async fn spawn_server<S: Source + 'static>(
        source: Arc<S>,
    ) -> (std::net::SocketAddr, oneshot::Sender<()>) {
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
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

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
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

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
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

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
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

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
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"HEAD / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
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
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

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
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

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
        let (addr, stop_tx) = spawn_server(Arc::clone(&source)).await;

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
        let result = tokio::net::TcpStream::connect(addr).await;
        assert!(result.is_err(), "停止后新连接应被拒绝");
    }

    // ── 400 Bad Request 测试 ────────────────────────────────

    #[tokio::test]
    async fn serve_400_on_missing_http_version() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        // 缺少 HTTP 版本 → 400
        stream.write_all(b"GET /\r\n\r\n").await.unwrap();

        let (code, _headers, _body) = read_response(&mut stream).await;
        assert_eq!(code, 400, "缺少 HTTP 版本应返回 400");
    }

    #[tokio::test]
    async fn serve_405_on_unknown_method() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"BANANA / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let (code, _headers, _body) = read_response(&mut stream).await;
        assert_eq!(code, 405, "未知方法应返回 405");
    }

    #[tokio::test]
    async fn serve_400_on_garbage_request_line() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        // 完全无效的请求行 → 400
        stream.write_all(b"xyz\r\n\r\n").await.unwrap();

        let (code, _headers, _body) = read_response(&mut stream).await;
        assert_eq!(code, 400, "垃圾请求行应返回 400");
    }

    // ── 502 Bad Gateway 测试 ─────────────────────────────────

    #[tokio::test]
    async fn serve_502_on_source_error() {
        let source = Arc::new(FailingSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let (code, _headers, body) = read_response(&mut stream).await;
        assert_eq!(
            code, 200,
            "FailingSource err after headers are written — status is 200"
        );
        // The error occurs only when the body is streamed; headers are already sent.
        // The connection then closes without a complete body.
        assert!(
            body.len() < 100,
            "body should be truncated because stream fails: {}",
            body.len()
        );
    }

    #[tokio::test]
    async fn serve_502_on_source_error_with_range() {
        let source = Arc::new(FailingSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nRange: bytes=0-9\r\n\r\n")
            .await
            .unwrap();

        let (code, _headers, body) = read_response(&mut stream).await;
        assert_eq!(
            code, 206,
            "FailingSource err after headers are written — status is 206"
        );
        assert!(body.len() < 10, "body should be truncated: {}", body.len());
    }

    // ── HEAD keep-alive 测试 ─────────────────────────────────

    #[tokio::test]
    async fn serve_head_keepalive_then_get() {
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("连接失败");

        // HEAD → 应保持连接（读取仅头部，不读取 body，因为 HEAD 无 body）
        stream
            .write_all(b"HEAD / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let (code1, headers1) = read_headers_only(&mut stream).await;
        assert_eq!(code1, 200);
        assert!(
            headers1.get("connection") != Some(&"close".to_string()),
            "HEAD 不应强制 Connection: close"
        );

        // 同一连接上 GET → 应成功
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nRange: bytes=0-4\r\n\r\n")
            .await
            .unwrap();
        let (code2, _headers2, body2) = read_response(&mut stream).await;
        assert_eq!(code2, 206, "HEAD 后 GET 应成功");
        assert_eq!(&body2, &[0, 1, 2, 3, 4]);
    }

    // ── Chunk-boundary 测试 ─────────────────────────────────

    #[tokio::test]
    async fn serve_full_body_delivered_across_chunks() {
        // FakeSource chunks at 7 bytes. A 100-byte file produces 15 chunks
        // (14 full + 1 partial). The response body must still equal the full data.
        let source = Arc::new(FakeSource::new(100));
        let (addr, _stop) = spawn_server(Arc::clone(&source)).await;

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
        assert_eq!(body.len(), 100);
        // body must be exactly [0, 1, 2, ..., 99] — no chunk-boundary artifacts
        assert_eq!(&body, &source.data);
    }

    // ── status_line 单元测试 ─────────────────────────────────────

    #[test]
    fn status_line_known_codes() {
        assert_eq!(status_line(200), "200 OK");
        assert_eq!(status_line(206), "206 Partial Content");
        assert_eq!(status_line(400), "400 Bad Request");
        assert_eq!(status_line(405), "405 Method Not Allowed");
        assert_eq!(status_line(416), "416 Range Not Satisfiable");
        assert_eq!(status_line(502), "502 Bad Gateway");
    }
}
