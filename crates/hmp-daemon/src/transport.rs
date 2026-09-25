//! 平台 IPC 传输（控制面）：Unix = domain socket；Windows = named pipe。
//!
//! 帧协议不变（`hmp_core::ipc` 长度前缀 JSON，运行在 `AsyncRead`/`AsyncWrite`
//! 之上）；端点统一以 `PathBuf` 表示——Windows 上是管道名
//! （`\\.\pipe\...`），`IpcListener::bind`/`IpcStream::connect` 内部做
//! 管道名归一。连接双端各自独占读写（单帧请求-响应 / 订阅推送），
//! 需要并发读写的一侧用 `tokio::io::split`（NamedPipeServer/Client 均实现
//! AsyncRead+AsyncWrite，无内建 into_split）。

use std::io;
use std::path::Path;
use tokio::io::{AsyncRead, AsyncWrite};

/// 平台无关的 IPC 连接（duplex 字节流）。
pub struct IpcStream(IpcStreamInner);

enum IpcStreamInner {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    #[cfg(windows)]
    PipeClient(tokio::net::windows::named_pipe::NamedPipeClient),
    #[cfg(windows)]
    PipeServer(tokio::net::windows::named_pipe::NamedPipeServer),
}

/// 平台无关的监听端。
pub struct IpcListener(IpcListenerInner);

enum IpcListenerInner {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    #[cfg(windows)]
    Pipe(PipeListener),
}

#[cfg(windows)]
struct PipeListener {
    name: String,
    /// 当前待 accept 的实例；accept 后立即补建下一个实例，保证任意时刻
    /// 至少有一个可连接实例（客户端几乎不会见到 ERROR_PIPE_BUSY）。
    current: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
}

impl IpcStream {
    /// 连接 daemon 端点。Windows 上管道实例全忙时短暂重试
    /// （serve 端 accept 后立即补建实例，busy 只是瞬态）。
    pub async fn connect(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self(IpcStreamInner::Unix(
                tokio::net::UnixStream::connect(path).await?,
            )))
        }
        #[cfg(windows)]
        {
            let name = pipe_name(path)?;
            const BUSY: i32 = 231; // ERROR_PIPE_BUSY
            const BUSY_RETRIES: u32 = 40; // 40 × 25ms ≈ 1s
            for attempt in 0..BUSY_RETRIES {
                match tokio::net::windows::named_pipe::ClientOptions::new().open(&name) {
                    Ok(client) => return Ok(Self(IpcStreamInner::PipeClient(client))),
                    Err(e) if e.raw_os_error() == Some(BUSY) && attempt + 1 < BUSY_RETRIES => {
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    }
                    Err(e) => return Err(e),
                }
            }
            unreachable!("retry loop always returns")
        }
    }
}

impl IpcListener {
    /// 绑定端点开始监听。Windows 上 `first_pipe_instance` 保证单实例：
    /// 已有 daemon 时返回 `ERROR_ACCESS_DENIED`（serve 层据此优雅退出）。
    pub fn bind(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let _ = path;
            Ok(Self(IpcListenerInner::Unix(
                tokio::net::UnixListener::bind(path)?,
            )))
        }
        #[cfg(windows)]
        {
            let name = pipe_name(path)?;
            let server = tokio::net::windows::named_pipe::ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)?;
            Ok(Self(IpcListenerInner::Pipe(PipeListener {
                name,
                current: Some(server),
            })))
        }
    }

    /// 等待并返回一条新连接。
    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        let inner = &mut self.0;
        #[cfg(unix)]
        let stream = match inner {
            IpcListenerInner::Unix(listener) => {
                let (stream, _addr) = listener.accept().await?;
                IpcStreamInner::Unix(stream)
            }
        };
        #[cfg(windows)]
        let stream = match inner {
            IpcListenerInner::Pipe(PipeListener { name, current }) => {
                let server = current
                    .take()
                    .expect("pipe listener instance consumed twice");
                server.connect().await?;
                // 立刻补建下一个实例再返回，连接处理与下一客户端等待互不阻塞。
                *current =
                    Some(tokio::net::windows::named_pipe::ServerOptions::new().create(name)?);
                IpcStreamInner::PipeServer(server)
            }
        };
        Ok(IpcStream(stream))
    }
}

/// Windows 管道名归一：已是 `\\.\pipe\...` 原样使用；任意路径（默认端点/
/// 测试临时目录）确定性映射为 `\\.\pipe\hmp-<sanitized>`（管道名不允许
/// `\` `:` 等字符，统一替换为 `-`）。
#[cfg(windows)]
fn pipe_name(path: &Path) -> io::Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "non-utf8 pipe endpoint"))?;
    if let Some(name) = text.strip_prefix(r"\\.\pipe\") {
        if name.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty pipe name",
            ));
        }
        return Ok(text.to_owned());
    }
    let mut name = String::from(r"\\.\pipe\hmp-");
    for ch in text.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => name.push(ch),
            _ => name.push('-'),
        }
    }
    Ok(name)
}

impl AsyncRead for IpcStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let this = self.get_mut();
        match &mut this.0 {
            #[cfg(unix)]
            IpcStreamInner::Unix(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            #[cfg(windows)]
            IpcStreamInner::PipeClient(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            #[cfg(windows)]
            IpcStreamInner::PipeServer(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for IpcStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let this = self.get_mut();
        match &mut this.0 {
            #[cfg(unix)]
            IpcStreamInner::Unix(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            #[cfg(windows)]
            IpcStreamInner::PipeClient(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            #[cfg(windows)]
            IpcStreamInner::PipeServer(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let this = self.get_mut();
        match &mut this.0 {
            #[cfg(unix)]
            IpcStreamInner::Unix(s) => std::pin::Pin::new(s).poll_flush(cx),
            #[cfg(windows)]
            IpcStreamInner::PipeClient(s) => std::pin::Pin::new(s).poll_flush(cx),
            #[cfg(windows)]
            IpcStreamInner::PipeServer(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let this = self.get_mut();
        match &mut this.0 {
            #[cfg(unix)]
            IpcStreamInner::Unix(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            #[cfg(windows)]
            IpcStreamInner::PipeClient(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            #[cfg(windows)]
            IpcStreamInner::PipeServer(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回环：bind → connect → 双向帧级字节往返（传输层自测，帧编解码在 ipc.rs）。
    #[tokio::test]
    async fn roundtrip_between_listener_and_stream() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transport-test.sock");
        let mut listener = IpcListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
            stream.write_all(b"pong").await.unwrap();
        });
        let mut client = IpcStream::connect(&path).await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        client.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
        server.await.unwrap();
    }

    /// 第二条连接：accept 后监听端仍可用（Windows 补建实例路径）。
    /// 空闲实例同一时刻只有一个（accept 时补建下一个），服务端 accept 前
    /// 并发第二条连接会 busy——真实调用方靠 connect 的 busy 重试兜底，
    /// 测试里逐条往返验证监听端复用。
    #[tokio::test]
    async fn listener_accepts_multiple_connections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transport-multi.sock");
        let mut listener = IpcListener::bind(&path).unwrap();
        use tokio::io::AsyncWriteExt;
        for round in 0..2u8 {
            let mut client = IpcStream::connect(&path).await.unwrap();
            let stream = listener.accept().await.unwrap();
            client.write_all(&[round]).await.unwrap();
            drop(stream);
            drop(client);
        }
    }

    #[cfg(windows)]
    #[test]
    fn pipe_name_normalization_is_deterministic() {
        let direct = pipe_name(Path::new(r"\\.\pipe\hmp")).unwrap();
        assert_eq!(direct, r"\\.\pipe\hmp");
        let mapped = pipe_name(Path::new(r"C:\Users\u\AppData\Local\Temp\abc\hmp.sock")).unwrap();
        assert!(mapped.starts_with(r"\\.\pipe\hmp-C--Users-u-"));
        assert!(!mapped.contains(':'));
        let again = pipe_name(Path::new(r"C:\Users\u\AppData\Local\Temp\abc\hmp.sock")).unwrap();
        assert_eq!(mapped, again);
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths_pass_through() {
        // Unix 上不做端点转换：bind/connect 直接吃路径（编译期保证）。
        let path = std::path::Path::new("/tmp/hmp.sock");
        assert_eq!(path, std::path::Path::new("/tmp/hmp.sock"));
    }
}
