//! 平台 IPC 传输（控制面）：Unix = domain socket；Windows = named pipe。
//!
//! 帧协议不变（`hmp_core::ipc` 长度前缀 JSON，运行在 `AsyncRead`/`AsyncWrite`
//! 之上）；端点统一以 `PathBuf` 表示——Windows 上是管道名
//! （`\\.\pipe\...`），`IpcListener::bind`/`IpcStream::connect` 内部做
//! 管道名归一。连接双端各自独占读写（单帧请求-响应 / 订阅推送），
//! 需要并发读写的一侧用 `tokio::io::split`（NamedPipeServer/Client 均实现
//! AsyncRead+AsyncWrite，无内建 into_split）。
//! Windows 服务端为预创建待命实例池（`PIPE_STANDBY`）：实例自 `CreateNamedPipe`
//! 起即 listening，池即并发连接坑位，单 accept 循环即可吸收并发连接
//! （AUDIT §16：单待命实例是裸连风暴 97% BUSY 的根因）；客户端在全部实例
//! 忙（ERROR_PIPE_BUSY）时于预算内指数退避。

use std::io;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(windows)]
use std::collections::VecDeque;
#[cfg(windows)]
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};

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

/// Windows 管道监听器：预创建待命实例池。
///
/// 管道实例自 `CreateNamedPipe` 起即处 listening 态（客户端 CreateFile 可
/// 直接占坑，无需服务端先 await connect）——池中每个实例都是一个并发连接
/// 坑位，单 accept 循环即可吸收并发连接。池按创建序存放、`accept` 取最老
/// （`pop_front`）；实例分配假设客户端 CreateFile 命中「最老」listening
/// 实例（FIFO，无官方文档佐证——由 transport/server 测试的「满池空闲态
/// 单客户端首连」场景守护，非 FIFO 会当场挂死，见 PLAN §2.2）。
#[cfg(windows)]
struct PipeListener {
    name: String,
    standby: VecDeque<NamedPipeServer>,
}

/// Windows 待命实例池深：桌面峰值并发（封面预取信号量 8 + 主订阅连接 +
/// CLI 偶发命令）的瞬时风暴由「池坑位 + 客户端 5s busy 退避」吸收。
#[cfg(windows)]
pub(crate) const PIPE_STANDBY: usize = 4;

/// 尽力补池至 [`PIPE_STANDBY`]。
/// - 补建失败但池非空：降级续跑（warn + 缩池）——资源紧张时缩容、缓解后
///   下一轮 accept 自动回满，这是监听器的自愈路径。
/// - 补建失败且池已空：返回原始错误（保留错误码供上层 fatal 分类，如
///   管道名被夺的 ACCESS_DENIED），监听器保持可重用——重试即再补建。
#[cfg(windows)]
fn refill_standby(name: &str, standby: &mut VecDeque<NamedPipeServer>) -> io::Result<()> {
    while standby.len() < PIPE_STANDBY {
        match ServerOptions::new().create(name) {
            Ok(server) => standby.push_back(server),
            Err(e) => {
                if standby.is_empty() {
                    return Err(e);
                }
                tracing::warn!(%e, len = standby.len(), "pipe standby pool refill degraded");
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Windows 客户端 BUSY 重试总预算（ERROR_PIPE_BUSY = 全部实例忙）。
/// 服务端池化（[`PIPE_STANDBY`]）为主修复，本预算为纵深：极端并发下排队
/// 重试而非即刻失败。serve.rs 单例探测用更短预算（见 `connect_with_budget`）。
const CONNECT_BUDGET: Duration = Duration::from_secs(5);

impl IpcStream {
    /// 连接 daemon 端点。Windows 上管道实例全忙（ERROR_PIPE_BUSY）时在
    /// 5s 预算内指数退避重试；其余错误（NotFound 等）即时返回，语义不变。
    pub async fn connect(path: &Path) -> io::Result<Self> {
        Self::connect_with_budget(path, CONNECT_BUDGET).await
    }

    /// 带预算的连接（crate 内部）。serve.rs 的单例探测用 250ms 短预算：
    /// 探测期间若他 daemon 的实例全部忙（池深 4 下罕见但存在），短退避后
    /// 即失败 → 走 bind → `first_pipe_instance` 撞 ACCESS_DENIED → 既有
    /// 「already running; exiting」让位路径，结果不变、路径更快。
    pub(crate) async fn connect_with_budget(path: &Path, budget: Duration) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let _ = budget; // Unix 无 busy 概念（backlog 排队），预算不适用
            Ok(Self(IpcStreamInner::Unix(
                tokio::net::UnixStream::connect(path).await?,
            )))
        }
        #[cfg(windows)]
        {
            let name = pipe_name(path)?;
            Ok(Self(IpcStreamInner::PipeClient(
                open_pipe_with_budget(&name, budget).await?,
            )))
        }
    }
}

/// ERROR_PIPE_BUSY。
#[cfg(windows)]
const BUSY: i32 = 231;

/// Windows 管道客户端：open 撞 BUSY 则按 [`busy_backoff`] 退避重试，总
/// 预算内仍忙才放弃（返回原始 BUSY 错误）；其余错误即时返回（NotFound
/// 语义不变——`hmp quit` 的幂等路径与 quit_no_spawn 回归依赖）。
#[cfg(windows)]
async fn open_pipe_with_budget(name: &str, budget: Duration) -> io::Result<NamedPipeClient> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut attempt: u32 = 0;
    loop {
        match ClientOptions::new().open(name) {
            Ok(client) => return Ok(client),
            Err(e) if e.raw_os_error() == Some(BUSY) => {
                let backoff = busy_backoff(attempt);
                if tokio::time::Instant::now() + backoff > deadline {
                    return Err(e); // 预算耗尽：上抛原始 BUSY 错误
                }
                tokio::time::sleep(backoff).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// BUSY 退避序列（纯函数，钉死策略）：10ms 起 ×2，封顶 250ms
/// （10/20/40/80/160/250/250/…）。shift 夹 5 仅防 `<<` 溢出且不改序列
/// （10<<5=320ms 已超封顶，外层 min 真正生效）。
#[cfg(windows)]
fn busy_backoff(attempt: u32) -> Duration {
    let shift = attempt.min(5);
    Duration::from_millis(10u64 << shift).min(Duration::from_millis(250))
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
            // 首实例持 first_pipe_instance（单实例裁决语义不变：已有 daemon 时
            // 返回 ERROR_ACCESS_DENIED，serve 层据此优雅退出）；首实例必须最先进
            // 池——池按创建序、accept 取最老，与客户端 FIFO 命中（见 PipeListener
            // 文档）对应。
            let first = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)?;
            let mut standby = VecDeque::with_capacity(PIPE_STANDBY);
            standby.push_back(first);
            // 尽力补满池：失败仅降级池深（bind 时池 ≥1，单实例语义只系于首个
            // 实例，故 bind 不因补建失败而失败）。
            let _ = refill_standby(&name, &mut standby);
            Ok(Self(IpcListenerInner::Pipe(PipeListener { name, standby })))
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
            IpcListenerInner::Pipe(PipeListener { name, standby }) => {
                // 先补池（尽力）：上一轮取走/丢弃的实例在此回补；池空且补建
                // 失败时原始错误上抛，监听器保持可重用——serve 的退避重试会
                // 再次补建（自愈）。
                refill_standby(name, standby)?;
                let server = standby
                    .pop_front()
                    .ok_or_else(|| io::Error::other("pipe standby pool empty"))?;
                match server.connect().await {
                    // 实例自创建起 listening：客户端早已 CreateFile 占坑的场合
                    // connect 立即返回（ERROR_PIPE_CONNECTED → Ok），正常负载
                    // 零等待。
                    Ok(()) => IpcStreamInner::PipeServer(server),
                    // connect 失败：实例随本臂 drop（不回池），serve 重试取下一
                    // 待命实例——实例级自愈。
                    Err(e) => return Err(e),
                }
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

    /// 第二条连接：accept 后监听端仍可用（Windows 池化路径：accept 取走一
    /// 个坑位、下一轮补回）。真实调用方靠池坑位 + busy 退避吸收并发，测试里
    /// 逐条往返验证监听端复用。
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

    /// 池化核心语义（AUDIT §16 97% BUSY 主修复）：bind 后**不调用 accept**，
    /// 并发连接全部立即占坑成功（Windows：实例自 CreateNamedPipe 起即
    /// listening，池深即坑位数；Unix：backlog 排队）。旧单实例实现在
    /// Windows 上第二个连接起耗尽 busy 预算失败。随后顺序 accept 收齐全部。
    /// 顺序 accept 侧同时守护「客户端命中最老 listening 实例（FIFO）」假设
    /// （PLAN §2.2）：若内核按 LIFO 分配，单客户端首连场景会挂死。
    #[tokio::test]
    async fn standby_pool_absorbs_concurrent_connects_without_accept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transport-pool.sock");
        let mut listener = IpcListener::bind(&path).unwrap();
        #[cfg(windows)]
        let count = PIPE_STANDBY;
        #[cfg(unix)]
        let count = 4; // Unix 无池：backlog 语义下同样应全部立连
        let mut clients = Vec::new();
        for _ in 0..count {
            let client = tokio::time::timeout(Duration::from_secs(2), IpcStream::connect(&path))
                .await
                .expect("并发连接应在 2s 内占坑成功（池化失效？）")
                .unwrap();
            clients.push(client);
        }
        // 顺序 accept 收齐：每个已占坑实例 connect() 立即返回。
        for i in 0..count {
            let accepted = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await;
            assert!(
                accepted.is_ok(),
                "accept 未在 2s 内收齐已占坑连接（第 {i} 个）"
            );
            accepted.unwrap().unwrap();
        }
        drop(clients);
    }

    /// 单实例裁决不被池化破坏：同名管道二次 bind 撞 first_pipe_instance →
    /// ERROR_ACCESS_DENIED（serve.rs「already running; exiting」依赖）。
    /// （tokio 1.53 的 create 需 reactor 上下文，用 #[tokio::test]。）
    #[cfg(windows)]
    #[tokio::test]
    async fn bind_denies_second_daemon_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transport-single.sock");
        let _first = IpcListener::bind(&path).unwrap();
        let err = match IpcListener::bind(&path) {
            Ok(_) => panic!("二次 bind 不应成功（单实例裁决被池化破坏？）"),
            Err(e) => e,
        };
        assert_eq!(
            err.raw_os_error(),
            Some(5), // ERROR_ACCESS_DENIED
            "二次 bind 应撞 first_pipe_instance: {err:?}"
        );
    }

    /// BUSY 退避序列（纯函数钉死策略，PLAN T2）：10ms 起 ×2、封顶 250ms、
    /// 大 attempt 不溢出。
    #[cfg(windows)]
    #[test]
    fn busy_backoff_progression() {
        assert_eq!(busy_backoff(0), Duration::from_millis(10));
        assert_eq!(busy_backoff(1), Duration::from_millis(20));
        assert_eq!(busy_backoff(2), Duration::from_millis(40));
        assert_eq!(busy_backoff(3), Duration::from_millis(80));
        assert_eq!(busy_backoff(4), Duration::from_millis(160));
        assert_eq!(
            busy_backoff(5),
            Duration::from_millis(250),
            "320 应封顶 250"
        );
        assert_eq!(
            busy_backoff(1000),
            Duration::from_millis(250),
            "大 attempt 钉在 250"
        );
    }

    /// 预算耗尽语义：占满全部待命实例（无 accept）→ 后续 open 全数 BUSY，
    /// 短预算内放弃并返回**原始 BUSY 错误**（错误码不吞）。
    #[cfg(windows)]
    #[tokio::test]
    async fn busy_budget_gives_up_and_returns_busy_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transport-busy.sock");
        let name = pipe_name(&path).unwrap();
        let listener = IpcListener::bind(&path).unwrap(); // 满池
        let mut hogs = Vec::new();
        for _ in 0..PIPE_STANDBY {
            hogs.push(ClientOptions::new().open(&name).unwrap()); // 占坑不 accept
        }
        let start = std::time::Instant::now();
        let err = open_pipe_with_budget(&name, Duration::from_millis(300))
            .await
            .unwrap_err();
        assert_eq!(
            err.raw_os_error(),
            Some(231),
            "预算耗尽应返回原始 BUSY: {err:?}"
        );
        assert!(
            start.elapsed() >= Duration::from_millis(100),
            "应经历退避而非立即放弃: {:?}",
            start.elapsed()
        );
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "应在预算附近放弃: {:?}",
            start.elapsed()
        );
        drop(hogs);
        drop(listener);
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
