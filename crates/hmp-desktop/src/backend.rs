//! 后端 IPC 适配层（M8）：Unix socket 客户端 + daemon 自动拉起。
//! UI 是 daemon 的又一个适配器（docs/PROJECT.md §8.6）：`Request` 命令、
//! `Subscribe` 推送 `DaemonState`；媒体库读按 CLI 契约直读 `library.sqlite3`。
//!
//! 线程模型：Slint 事件循环独占 UI 线程（slint 对象非 Send）；本模块全部
//! IPC 运行在进程内 tokio 多线程 runtime，回 UI 一律经
//! `slint::invoke_from_event_loop`（回调闭包与携带数据都满足 Send）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hmp_core::ipc::{DaemonState, Event, MAX_FRAME, Request, Response, decode_frame, encode_frame};
use hmp_core::{QueueEntry, TrackProvider};
use hmp_daemon::transport::IpcStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// spawn 后等待 daemon 就绪：~10 次 × 300ms（CLI `wait_for_socket` 同量级）。
const SPAWN_RETRIES: u32 = 10;
const RETRY_INTERVAL: Duration = Duration::from_millis(300);
/// 订阅断线（daemon 退出/重启）后的重连间隔。
const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
/// 队列分页单页大小（帧上限 1MB，远未触及；大队列自动翻页）。
const QUEUE_PAGE_LIMIT: usize = 1000;

/// 后端适配错误（离线降级的判定输入）。手写 Display/Error impl——
/// hmp-desktop 依赖面不含 thiserror（进程内 UI 适配层，错误只进日志）。
#[derive(Debug)]
pub enum BackendError {
    Io(std::io::Error),
    /// 找不到可拉起的 `hmp` 后端二进制（current_exe 同目录与 PATH 均无）。
    NoBackendBinary,
    /// daemon 拉起后在退避窗口内未就绪。
    SpawnTimeout,
    /// 协议错误（畸形帧/连接中断）。
    Protocol(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::NoBackendBinary => write!(
                f,
                "找不到可拉起的 hmp 后端二进制（current_exe 同目录与 PATH 均无）"
            ),
            Self::SpawnTimeout => write!(f, "daemon 启动超时"),
            Self::Protocol(m) => write!(f, "protocol error: {m}"),
        }
    }
}

impl std::error::Error for BackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for BackendError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// 桌面进程内 tokio 多线程 runtime：UI 线程只跑 Slint 事件循环，
/// 全部 IPC（短连接请求 + 订阅长连接）在本 runtime 上执行。
pub struct BackendRuntime {
    rt: tokio::runtime::Runtime,
}

impl BackendRuntime {
    /// 构建多线程 runtime（enable_all：IO + 时间驱动）。
    pub fn new() -> Result<Self, std::io::Error> {
        Ok(Self {
            rt: tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?,
        })
    }

    /// 派生后台任务（订阅循环 / 单发命令）。失败即丢弃（命令-查询分离：
    /// 命令只描述意图，状态以 daemon 推送为准）。
    pub fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.rt.spawn(fut);
    }
}

/// 连接 daemon 端点（短连接；路径与 daemon/CLI 同一实现，勿重复实现）。
async fn connect() -> Result<IpcStream, BackendError> {
    Ok(IpcStream::connect(&hmp_daemon::server::socket_path()).await?)
}

/// 连接或拉起后端（镜像 CLI `connect_or_spawn` 语义）：
/// ENOENT / ECONNREFUSED → 解析 `hmp` 二进制（current_exe 同目录 → PATH），
/// 经 `serve::spawn_detached_exe` 拉起 `hmp serve --background`，退避重试；
/// 找不到二进制或重试耗尽 → Err（调用方降级离线模式）。
pub async fn connect_or_spawn() -> Result<IpcStream, BackendError> {
    match connect().await {
        Ok(stream) => Ok(stream),
        Err(BackendError::Io(e))
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            if e.kind() == std::io::ErrorKind::ConnectionRefused {
                // 残留 socket（daemon 自身持锁也会再清理，双保险同 CLI）；
                // Windows 管道不落盘，remove 失败无碍。
                let _ = std::fs::remove_file(hmp_daemon::server::socket_path());
            }
            let exe = resolve_backend_binary().ok_or(BackendError::NoBackendBinary)?;
            hmp_daemon::serve::spawn_detached_exe(&exe, &["serve", "--background"])
                .map_err(BackendError::Io)?;
            for _ in 0..SPAWN_RETRIES {
                tokio::time::sleep(RETRY_INTERVAL).await;
                if let Ok(stream) = connect().await {
                    return Ok(stream);
                }
            }
            Err(BackendError::SpawnTimeout)
        }
        Err(e) => Err(e),
    }
}

/// 解析 `hmp` 后端二进制：优先 current_exe 同目录（cargo 构建布局），
/// 回退 PATH 逐目录扫描。桌面进程自身是 hmp-desktop，不能像 CLI 那样
/// 用 current_exe 自启（`serve` 参数对测试二进制/桌面二进制无意义）。
pub fn resolve_backend_binary() -> Option<PathBuf> {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        let candidate = dir.join(backend_binary_name());
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let path = std::env::var_os("PATH")?;
    find_hmp_in_dirs(std::env::split_paths(&path))
}

/// 平台后端二进制名（Windows 上是 hmp.exe）。
fn backend_binary_name() -> String {
    format!("hmp{}", std::env::consts::EXE_SUFFIX)
}

/// 在候选目录序列中找 `hmp` 可执行文件（纯逻辑；与 PATH 扫描同惯例，
/// 不再校验 x 位）。
fn find_hmp_in_dirs<I: Iterator<Item = PathBuf>>(dirs: I) -> Option<PathBuf> {
    dirs.map(|dir| dir.join(backend_binary_name()))
        .find(|candidate| candidate.is_file())
}

/// 单发请求-响应（每命令一条短连接，CLI `request()` 同模式）。
/// 命令-查询分离：`Response::Ok` 仅代表受理，真实结果经订阅事件呈现。
///
/// 冷启动窗口重试：订阅循环首连经 `connect_or_spawn` 拉起 daemon 需
/// ~0.3-3s，此窗口内的命令短连接会得到 NotFound/Refused——逐次退避重试
/// （不拉起：daemon 生命周期归订阅循环，命令路径不重复 spawn），耗尽才
/// 报错（调用方降级提示，不再静默丢弃点击）。
pub async fn request(req: Request) -> Result<Response, BackendError> {
    /// NotFound/Refused 的重试次数（×250ms ≈ 1s，覆盖常规冷启动窗口）。
    const CONNECT_RETRIES: u32 = 4;
    const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(250);
    let mut stream = None;
    for attempt in 0..=CONNECT_RETRIES {
        match connect().await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(BackendError::Io(e))
                if (e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::ConnectionRefused)
                    && attempt < CONNECT_RETRIES =>
            {
                tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
            }
            Err(e) => return Err(e),
        }
    }
    let Some(mut stream) = stream else {
        return Err(BackendError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "daemon unreachable after retries",
        )));
    };
    write_frame(&mut stream, &req).await?;
    let Some(frame) = read_frame(&mut stream).await? else {
        return Err(BackendError::Protocol("daemon closed connection".into()));
    };
    let response =
        decode_frame::<Response>(&frame).map_err(|e| BackendError::Protocol(e.to_string()));
    if let Ok(Response::Err { code, message }) = &response {
        // 拒绝型响应（未登录/越界等）此前被 `let _ =` 静默吞掉——UI「点了
        // 没反应」的主要来源之一；至少留一条 warn 级日志可查。
        tracing::warn!(?code, %message, "daemon rejected request");
    }
    response
}

/// 订阅事件（跨线程投递到 UI 线程；全部字段 Send）。
pub struct UiStateEvent {
    /// daemon 复合状态快照；`None` = 离线（连接失败或 daemon 已退出）。
    pub state: Option<DaemonState>,
    /// 队列结构变化时的全量投影行；`None` = 结构未变，复用现有模型
    /// （position 推送 ~10Hz，绝不随它重建队列模型）。
    pub queue_rows: Option<Vec<QueueRowMeta>>,
    /// 媒体库内容变更（`Event::LibraryChanged`；UI 重查 sqlite 刷新库页）。
    pub library_changed: bool,
}

/// 队列行投影（IPC 纯 ID + 媒体库元数据）。`TrackRow` 组装在 UI 线程——
/// `slint::Image` 非 Send，封面只能在 UI 线程构造。
#[derive(Clone, Debug)]
pub struct QueueRowMeta {
    /// 源键：QQ songmid / `local:<路径>`。
    pub mid: String,
    pub title: String,
    /// 展示串（"A / B"）。
    pub artists: String,
    pub album: String,
    /// 毫秒（媒体库未缓存 → 0）。
    pub duration_ms: i32,
    /// 媒体库封面 URI（本地 file:// 或 QQ 远程 URL；QQ 远程 UI 禁直连，
    /// 经 `CoverGet` IPC 换本地产物）。
    pub cover_uri: Option<String>,
}

/// 订阅循环：首连走 [`connect_or_spawn`]（拉起 daemon）；后续断线重连只做
/// 普通连接——用户显式 `hmp quit` 后 UI 不得反复把 daemon 拉活。
/// 每条 `Event::StateChanged` 投递一次回调（经 `invoke_from_event_loop` 在
/// UI 线程执行）；连接失败周期性投递离线事件（状态翻转时一次，不重复刷写）。
/// 事件帧解码失败：记 error 日志并断线走退避重连（协议版本错配/帧损坏
/// 均不静默跳过，否则 UI 状态冻结且无从排查）。
pub fn spawn_state_subscription(runtime: &BackendRuntime, on_event: ArcUiStateHandler) {
    runtime.spawn(async move {
        let handler = on_event;
        // 队列结构版本（QueueSummary.revision：结构变更 +1，position tick 不动）。
        let mut last_revision: Option<u64> = None;
        let mut offline_reported = false;
        loop {
            // 首连（last_revision 尚无观测）允许自动拉起 daemon，其余只重连。
            let connected = if last_revision.is_none() {
                connect_or_spawn().await
            } else {
                connect().await
            };
            let mut stream = match connected {
                Ok(stream) => stream,
                Err(_) => {
                    if !offline_reported {
                        offline_reported = true;
                        dispatch_ui(
                            &handler,
                            UiStateEvent {
                                state: None,
                                queue_rows: Some(Vec::new()),
                                library_changed: false,
                            },
                        );
                    }
                    tokio::time::sleep(RECONNECT_INTERVAL).await;
                    continue;
                }
            };
            offline_reported = false;
            // 订阅：server 先推初始快照；重连后首帧强制重建队列模型
            // （daemon 重启后 revision 归零，仅靠版本比对会漏重建）。
            if write_frame(&mut stream, &Request::Subscribe).await.is_err() {
                tokio::time::sleep(RECONNECT_INTERVAL).await;
                continue;
            }
            last_revision = None;
            while let Ok(Some(frame)) = read_frame(&mut stream).await {
                // 解码失败一律断线重连（break 走循环尾的既有退避路径），不
                // 静默跳过：单帧损坏无法界定损伤范围，而 `missing field` /
                // `unknown variant` 意味着协议版本错配（升级窗口内旧 daemon
                // 仍占端点），后续帧同样解不出来——跳过会让 UI 状态永久冻结
                // 且无从排查。不区分「可恢复单帧损坏」与「版本错配」是取简
                // 单方案：热循环重连与 daemon 不可达同路径（2s 退避），可接受。
                let event = match decode_frame::<Event>(&frame) {
                    Ok(event) => event,
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            frame_len = frame.len(),
                            "undecodable event frame from daemon; reconnecting"
                        );
                        break;
                    }
                };
                match event {
                    Event::LibraryChanged => {
                        // 库内容变更：轻量信号，不触发队列重建（队列结构由
                        // StateChanged.revision 驱动）。
                        dispatch_ui(
                            &handler,
                            UiStateEvent {
                                state: None,
                                queue_rows: None,
                                library_changed: true,
                            },
                        );
                    }
                    Event::StateChanged(state) => {
                        // 队列结构变化 → 重拉队列并投影（revision 不随 position tick 前进）。
                        let queue_rows = if last_revision != Some(state.queue.revision) {
                            last_revision = Some(state.queue.revision);
                            Some(project_queue().await)
                        } else {
                            None
                        };
                        dispatch_ui(
                            &handler,
                            UiStateEvent {
                                state: Some(state),
                                queue_rows,
                                library_changed: false,
                            },
                        );
                    }
                }
            }
            // EOF（daemon 退出）/ 读错误：断线，退避重连。
            tokio::time::sleep(RECONNECT_INTERVAL).await;
        }
    });
}

/// 投递到 UI 线程执行（slint 对象非 Send，订阅任务线程绝不直接写 UI；
/// 事件循环未启动时事件排队，`ui.run()` 后逐个应用）。事件循环已拆除
/// （应用退出）→ 丢弃。
fn dispatch_ui(handler: &ArcUiStateHandler, event: UiStateEvent) {
    let handler = Arc::clone(handler);
    let _ = slint::invoke_from_event_loop(move || handler(event));
}

/// UI 侧事件处理器（在 UI 线程被调用；Send+Sync 以便跨线程克隆投递）。
pub type ArcUiStateHandler = Arc<dyn Fn(UiStateEvent) + Send + Sync>;

/// 队列投影：`QueueList` 全量分页 → 媒体库批量元数据（直读
/// `data_dir()/library.sqlite3`，CLI `queue list` 同契约）。
/// 元数据未命中的曲目（如 CLI 搜索来播的 QQ 曲目）标题回退为 id、时长 0。
async fn project_queue() -> Vec<QueueRowMeta> {
    let mut entries: Vec<QueueEntry> = Vec::new();
    loop {
        let req = Request::QueueList {
            offset: entries.len(),
            limit: QUEUE_PAGE_LIMIT,
        };
        match request(req).await {
            Ok(Response::QueueList(page)) => {
                let empty = page.items.is_empty();
                entries.extend(page.items);
                // 空页（越界）或取满 total：分页结束。
                if empty || entries.len() >= page.total {
                    break;
                }
            }
            _ => return Vec::new(),
        }
    }
    let ids: Vec<String> = entries.iter().map(|e| e.track_id.to_string()).collect();
    // 阻塞 rusqlite：查询毫秒级，直接在当前 runtime 线程执行（daemon 服务器
    // 同样在 async 上下文持 std Mutex 操作 LibraryDb）；投影线程专用连接，
    // 用完即弃，不与 daemon 的连接共享（rusqlite 连接不可跨线程共用）。
    let meta = query_library_meta(&ids);
    entries
        .into_iter()
        .map(|entry| projected_row(entry.track_id.to_string(), &meta))
        .collect()
}

/// 单条队列投影：媒体库未命中 → 标题回退 id、歌手/专辑空、时长 0
/// （`hmp queue list` 同语义）；纯逻辑，测试覆盖回退路径。
fn projected_row(id: String, meta: &HashMap<String, ProjectedMeta>) -> QueueRowMeta {
    match meta.get(&id) {
        Some(m) => QueueRowMeta {
            duration_ms: m
                .duration_ms
                .map(|ms| ms.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
                .unwrap_or(0),
            mid: id,
            title: m.title.clone(),
            artists: m.artist.clone().unwrap_or_default(),
            album: m.album.clone().unwrap_or_default(),
            cover_uri: m.cover_uri.clone(),
        },
        None => QueueRowMeta {
            title: id.clone(),
            mid: id,
            artists: String::new(),
            album: String::new(),
            duration_ms: 0,
            cover_uri: None,
        },
    }
}

/// 投影元数据（`TrackMeta` 扩列投影：含时长/封面，AUDIT §8.11）。
struct ProjectedMeta {
    title: String,
    artist: Option<String>,
    album: Option<String>,
    duration_ms: Option<i64>,
    cover_uri: Option<String>,
}

/// 媒体库批量投影查询（阻塞）：`track_meta_batch` 一次拿标题/歌手/专辑/
/// 时长/封面（storage 扩列后 QQ stub 缓存的时长有了读出口，队列行不再
/// 显示 0:00）。库缺失/不可用返回空表（全部回退）。
fn query_library_meta(ids: &[String]) -> HashMap<String, ProjectedMeta> {
    let mut meta: HashMap<String, ProjectedMeta> = HashMap::new();
    let Ok(mut db) = hmp_storage::LibraryDb::open(&hmp_storage::data_dir().join("library.sqlite3"))
    else {
        return meta;
    };
    let mut qq = Vec::new();
    let mut local = Vec::new();
    for id in ids {
        if TrackProvider::from_id(id) == TrackProvider::Local {
            local.push(id.clone());
        } else {
            qq.push(id.clone());
        }
    }
    let mut absorb = |rows: Vec<hmp_storage::TrackMeta>| {
        for m in rows {
            meta.insert(
                m.source_key.clone(),
                ProjectedMeta {
                    title: m.title,
                    artist: m.artist,
                    album: m.album,
                    duration_ms: m.duration_ms,
                    cover_uri: m.cover_uri,
                },
            );
        }
    };
    if let Ok(rows) = db.track_meta_batch("qq", &qq) {
        absorb(rows);
    }
    if let Ok(rows) = db.track_meta_batch("local", &local) {
        absorb(rows);
    }
    meta
}

/// 写一帧请求（长度前缀 JSON；与 daemon server 同契约）。
/// 只服务 `Request`：不引入 serde 直依赖（serde_json 对具体类型即可编码）。
async fn write_frame(stream: &mut IpcStream, req: &Request) -> std::io::Result<()> {
    let frame = encode_frame(req).map_err(|e| std::io::Error::other(e.to_string()))?;
    stream.write_all(&frame).await
}

/// 读一帧（含 4 字节长度前缀）；EOF 返回 `None`。
async fn read_frame(stream: &mut IpcStream) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME - 4 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;
    let mut frame = Vec::with_capacity(4 + len);
    frame.extend_from_slice(&len_buf);
    frame.extend_from_slice(&payload);
    Ok(Some(frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_hmp_in_dirs_picks_existing_file() {
        let dir = std::env::temp_dir().join(format!("hmp-desktop-bin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 平台二进制名（Windows 上是 hmp.exe），与 find_hmp_in_dirs 同一约定。
        let bin_name = format!("hmp{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(dir.join(&bin_name), b"").unwrap();
        let found = find_hmp_in_dirs([dir.clone(), std::env::temp_dir()].into_iter());
        assert_eq!(found, Some(dir.join(&bin_name)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn find_hmp_in_dirs_misses_when_absent() {
        let empty = std::env::temp_dir().join(format!("hmp-desktop-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(find_hmp_in_dirs([empty].into_iter()), None);
    }

    #[test]
    fn projected_row_falls_back_to_id_when_meta_missing() {
        // 媒体库未命中（CLI 搜来播的 QQ 曲目）：标题回退 id，其余空。
        let meta = HashMap::new();
        let row = projected_row("003Z3i2C0a1b2c".into(), &meta);
        assert_eq!(row.mid, "003Z3i2C0a1b2c");
        assert_eq!(row.title, "003Z3i2C0a1b2c");
        assert_eq!(row.artists, "");
        assert_eq!(row.album, "");
        assert_eq!(row.duration_ms, 0);
    }

    #[test]
    fn projected_row_uses_library_meta_when_hit() {
        let mut meta = HashMap::new();
        meta.insert(
            "mid-1".to_string(),
            ProjectedMeta {
                title: "开始懂了".into(),
                artist: Some("孙燕姿".into()),
                album: None,
                duration_ms: None,
                cover_uri: None,
            },
        );
        let row = projected_row("mid-1".into(), &meta);
        assert_eq!(row.title, "开始懂了");
        assert_eq!(row.artists, "孙燕姿");
        assert_eq!(row.album, "");
        // 库中时长缺失 → 0（不为负/不为垃圾值）。
        assert_eq!(row.duration_ms, 0);
    }

    #[test]
    fn projected_row_carries_local_duration_and_clamps() {
        let mut meta = HashMap::new();
        meta.insert(
            "local:/a.flac".to_string(),
            ProjectedMeta {
                title: "本地曲".into(),
                artist: None,
                album: Some("本地专辑".into()),
                duration_ms: Some(215_000),
                cover_uri: Some("file:///covers/a.jpg".into()),
            },
        );
        meta.insert(
            "local:/b.flac".to_string(),
            ProjectedMeta {
                title: "异常时长".into(),
                artist: None,
                album: None,
                duration_ms: Some(i64::MAX),
                cover_uri: None,
            },
        );
        let row = projected_row("local:/a.flac".into(), &meta);
        assert_eq!(row.duration_ms, 215_000);
        assert_eq!(
            row.cover_uri.as_deref(),
            Some("file:///covers/a.jpg"),
            "扩列投影应带出封面 URI"
        );
        let row = projected_row("local:/b.flac".into(), &meta);
        assert_eq!(row.duration_ms, i32::MAX);
    }
}
