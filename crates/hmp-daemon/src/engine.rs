//! 播放引擎：命令循环 + 队列裁决 + 自动续播 + 复合状态发布（spec §4.2 `daemon.rs`）。
//!
//! 单一命令通道：所有输入适配器（socket 服务器 / tray / MPRIS）把
//! [`Request`] 发进 [`EngineHandle::command_tx`]，由引擎串行处理；
//! 单一状态出口：`watch<DaemonState>`。Next/Previous 由引擎拦截做队列
//! 导航（PlayerCore 忽略这两个命令，见 hmp-player core.rs）。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use hmp_core::{
    DaemonState, ErrorInfo, IpcErrorCode, LoadRequest, PlayRequest, PlaybackCapabilities,
    PlaybackState, PlaybackStatus, PlayerCommand, PlayerEvent, QueueSnapshot, Request, TrackId,
};
use tokio::sync::{Mutex, mpsc, watch};

use crate::player::{EngineError, PlaybackDriver, ResolvedTrack, SourceResolver};

/// 会话持久化文件内容（`$XDG_DATA_HOME/hmp/playback_state.json`）。
/// 恢复 queue/volume/position；恢复后不自动播放，首次 Play 时续播（里程碑 D）。
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionFile {
    /// 队列完整内部状态（restore_state 直接还原）。
    queue: hmp_core::queue::QueueState,
    /// 音量 0.0..=1.0。
    volume: f64,
    /// 当前曲播放位置（毫秒；has_current 时有效）。
    position_ms: u64,
}

/// 读取会话文件（不存在/损坏 → None，不报错）。
fn read_session_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Option<SessionFile>> {
    let path = path.as_ref();
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(f) => Ok(Some(f)),
            Err(e) => {
                tracing::warn!(%e, "corrupt session file; ignoring");
                Ok(None)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// 原子写会话文件（tmp + rename）。
fn write_session_file(path: &std::path::Path, f: &SessionFile) -> std::io::Result<()> {
    let json = serde_json::to_vec(f).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 持久化镜像（脏检查基准）。
#[derive(Default)]
struct SessionMirror {
    queue_rev: u64,
    volume: f64,
    /// (曲目 id, 位置 ms) 上次写入值。
    position: Option<(TrackId, u64)>,
    /// 上次写盘时刻（位置节流用）。
    last_write: Option<std::time::Instant>,
    /// 上次写盘时的播放状态（翻转 → 立即写）。
    playing: bool,
}

/// 启动时恢复的会话上下文。
struct RestoredSession {
    /// 恢复的当前曲（Play 时若装载同曲则 Seek 续播）。
    current: TrackId,
    position_ms: u64,
}

/// 最近一次成功装载的完整信息（失败回滚用）。
#[derive(Clone)]
struct AppliedLoad {
    track: hmp_core::Track,
    uri: String,
    quality: hmp_core::AudioQuality,
    load_gen: u64,
}

/// 预解析槽（G2 gapless）：后台 resolve 队列下一首，曲间切换时直接消费。
/// `key = (队列 revision, 装载代际)`：写槽防乱序（旧任务不得覆盖新任务，
/// 字典序比较）；**消费**只校验代际与曲目 id——队列导航（skip_next/
/// advance_on_eos）本身会 bump revision，若把 revision 纳入消费条件，
/// 缓存永远无法命中（曲目 id 已保证不会消费错曲）。
struct PreloadSlot {
    /// 触发时指纹：(队列 revision, 装载代际)。
    key: (u64, u64),
    /// 预解析的目标曲目（消费时须与请求 id 一致）。
    id: TrackId,
    /// 预解析结果（含解密代理 guard；消费时 move 移交）。
    res: ResolvedTrack,
}

/// 当前播放会话（媒体库写回锚点：event id 精确闭合）。
#[derive(Clone)]
struct PlaybackSession {
    track_id: i64,
    event_id: i64,
}

/// 引擎句柄（服务器 / tray / MPRIS 持有；可 Clone）。
#[derive(Clone)]
pub struct EngineHandle {
    /// 命令通道（唯一输入）。
    pub command_tx: mpsc::UnboundedSender<Request>,
    /// 复合状态（唯一输出）。
    pub state_rx: watch::Receiver<DaemonState>,
    /// 凭证前置校验（服务器对 Play 类请求同步检查，spec §6）。
    pub credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
    /// 引擎终止信号（sticky watch：`run()` 退出时置 true；serve 据此优雅退出清理 socket，spec §6）。
    pub terminated: watch::Receiver<bool>,
    /// 播放能力（MPRIS CanGoNext/CanGoPrevious，随 publish 同步发布，Finding 9）。
    pub caps_rx: watch::Receiver<PlaybackCapabilities>,
    /// 完整队列（结构变更时更新；消费方：server 的 Queue/QueueList）。
    pub queue_rx: watch::Receiver<QueueSnapshot>,
    /// 媒体库（server 直操作：收藏/歌单写命令；daemon 层注入）。
    pub library: Option<std::sync::Arc<std::sync::Mutex<hmp_storage::LibraryDb>>>,
    /// QQ 同步 worker 触发句柄（daemon 层注入）。
    pub sync_handle: Option<crate::sync::SyncHandle>,
    /// 评论服务（daemon 层注入；未注入时评论命令报不可用）。
    pub comment: Option<crate::comment::CommentService>,
    /// 内容读服务（daemon 层注入；未注入时搜索/歌词/账号/封面命令报不可用）。
    pub content: Option<crate::content::ContentService>,
    /// 扫码登录服务（daemon 层注入；未注入时 LoginQr*/Logout 命令报不可用）。
    pub login: Option<Arc<crate::login::LoginService>>,
    /// 媒体库变更代际（watcher/sync/写命令 bump；server 据此推
    /// `Event::LibraryChanged`，客户端重查 sqlite——直读契约的刷新信号）。
    pub library_tx: watch::Sender<u64>,
    pub library_rx: watch::Receiver<u64>,
}

impl EngineHandle {
    /// 发送请求（命令-查询分离：仅返回是否投递成功）。
    pub async fn cmd(&self, req: Request) -> Result<(), mpsc::error::SendError<Request>> {
        self.command_tx.send(req)
    }
}

/// 播放引擎。
pub struct PlaybackEngine {
    driver: Arc<dyn PlaybackDriver>,
    resolver: Arc<dyn SourceResolver>,
    queue: hmp_core::QueueCore,
    state_tx: watch::Sender<DaemonState>,
    state_rx: watch::Receiver<PlaybackState>,
    cmd_rx: mpsc::UnboundedReceiver<Request>,
    active_media: Option<hmp_media::PreparedMedia>,
    /// 命令代际（换曲操作执行前置位，Finding 1）。
    seq: u64,
    /// 最近一次命令错误（解析失败等；成功换曲时清空，Finding 2）。
    last_error: Option<ErrorInfo>,
    /// 播放引擎阶段（spec §7 状态机）。
    phase: hmp_core::EnginePhase,
    /// 装载代际（每次 load_and_play 递增；旧代事件过滤，spec §7 换代机制）。
    current_gen: u64,
    /// 播放能力发布（MPRIS 订阅，Finding 9）。
    caps_tx: watch::Sender<PlaybackCapabilities>,
    /// 终止信号发布（sticky，Finding 7）。
    term_tx: watch::Sender<bool>,
    /// 媒体库（播放会话写库；不可用时为 None，播放不阻断）。
    library: Option<std::sync::Arc<std::sync::Mutex<hmp_storage::LibraryDb>>>,
    /// 当前播放会话（媒体库写回锚点：event id 精确闭合）。
    session: Option<PlaybackSession>,
    /// 最近一次成功装载（失败回滚用）。
    last_load: Option<AppliedLoad>,
    /// 下一首预解析缓存（G2；后台任务写、load_and_play 消费）。
    preload_slot: Arc<Mutex<Option<PreloadSlot>>>,
    /// 用户音量（RG 补偿前；SetVolume 更新，换曲时叠加当前曲增益）。
    user_volume: f64,
    /// 当前曲 ReplayGain 增益因子（无标签/关闭 = 1.0）。
    rg_factor: f64,
    /// 当前曲 ReplayGain 标签增益（dB 原值，未 clamp；无标签 → None）。
    /// 打磨：随 DaemonState 发布供 CLI status 展示。
    current_rg_db: Option<f64>,
    /// 等待驱动应用装载的超时（测试注入短超时）。
    load_timeout: std::time::Duration,
    /// 完整队列快照（仅结构变化时发送；position tick 不触发——O(1) publish）。
    queue_tx: watch::Sender<QueueSnapshot>,
    /// 上次发布的队列版本（避免重复发送）。
    last_queue_rev: u64,
    /// 会话持久化路径（None = 不持久化）。
    session_path: Option<std::path::PathBuf>,
    /// 位置写盘节流。
    persist_throttle: std::time::Duration,
    /// 上次写盘时的内存镜像（脏检查）。
    saved: SessionMirror,
    /// 恢复的会话（Play 时应用 seek；应用后清除）。
    restored: Option<RestoredSession>,
}

impl PlaybackEngine {
    /// 启动引擎（spawn 主循环任务），返回句柄。
    pub fn start(
        driver: Arc<dyn PlaybackDriver>,
        resolver: Arc<dyn SourceResolver>,
        credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> EngineHandle {
        Self::start_with_library(driver, resolver, credential_ok, None, None)
    }

    /// 启动引擎并挂载媒体库（B4：播放会话写库）。
    /// `session_path`：会话持久化路径（None = 不持久化）。
    pub fn start_with_library(
        driver: Arc<dyn PlaybackDriver>,
        resolver: Arc<dyn SourceResolver>,
        credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
        library: Option<std::sync::Arc<std::sync::Mutex<hmp_storage::LibraryDb>>>,
        session_path: Option<std::path::PathBuf>,
    ) -> EngineHandle {
        Self::start_with_options(
            driver,
            resolver,
            credential_ok,
            library,
            std::time::Duration::from_secs(5),
            session_path,
            std::time::Duration::from_secs(5),
        )
    }

    /// 带装载超时注入的启动（测试用短超时驱动失败路径）。
    /// `session_path`：会话持久化路径（None = 不持久化）；`persist_throttle`：位置写盘节流。
    pub fn start_with_options(
        driver: Arc<dyn PlaybackDriver>,
        resolver: Arc<dyn SourceResolver>,
        credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
        library: Option<std::sync::Arc<std::sync::Mutex<hmp_storage::LibraryDb>>>,
        load_timeout: std::time::Duration,
        session_path: Option<std::path::PathBuf>,
        persist_throttle: std::time::Duration,
    ) -> EngineHandle {
        // 启动恢复：队列/音量/位置（不自动播放；Play 时续播，里程碑 D）。
        // 同步读取（run 开始前），避免与首帧发布竞态。
        let session_file = match &session_path {
            Some(p) => match read_session_file(p) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(%e, "failed to read session file");
                    None
                }
            },
            None => None,
        };
        let mut queue = hmp_core::QueueCore::new();
        let restored_volume = session_file.as_ref().map(|f| f.volume);
        let restored = match session_file {
            Some(f) => {
                let SessionFile {
                    queue: q,
                    volume: _,
                    position_ms,
                } = f;
                queue.restore_state(q);
                // 仅在有当前曲时保留续播上下文（无当前曲只恢复队列/音量）。
                queue.current().cloned().map(|cur| RestoredSession {
                    current: cur,
                    position_ms,
                })
            }
            None => None,
        };
        if let Some(v) = restored_volume {
            driver.set_volume(v);
        }
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (state_tx, state_rx) = watch::channel(DaemonState::default());
        let playback_rx = driver.subscribe_state();
        let (caps_tx, caps_rx) = watch::channel(PlaybackCapabilities::default());
        let (queue_tx, queue_rx) = watch::channel(QueueSnapshot::default());
        // sticky 终止信号：晚到的接收者立即可见（watch 保留当前值，Finding 7）。
        let (term_tx, term_rx) = watch::channel(false);
        // 库变更代际（daemon 层可整体替换为贯穿 watcher/sync 的通道）。
        let (library_tx, library_rx) = watch::channel(0u64);
        let queue_rev_after_restore = queue.revision();
        let mut engine = Self {
            driver,
            resolver,
            queue,
            state_tx,
            state_rx: playback_rx,
            cmd_rx,
            active_media: None,
            seq: 0,
            last_error: None,
            phase: hmp_core::EnginePhase::Idle,
            current_gen: 0,
            caps_tx,
            term_tx,
            library,
            session: None,
            last_load: None,
            preload_slot: Arc::new(Mutex::new(None)),
            user_volume: restored_volume.unwrap_or(1.0),
            rg_factor: 1.0,
            current_rg_db: None,
            load_timeout,
            queue_tx,
            last_queue_rev: 0,
            session_path,
            persist_throttle,
            // 镜像初始化为恢复后状态：首帧 publish 不因 rev/volume 脏而覆写
            // 文件（避免启动即退出时丢失恢复值；音量经 driver 异步生效前的窗口）。
            saved: SessionMirror {
                queue_rev: queue_rev_after_restore,
                volume: restored_volume.unwrap_or(1.0),
                ..SessionMirror::default()
            },
            restored,
        };
        tokio::spawn(async move {
            engine.run().await;
            // 退出前最终写（保真位置/队列）；写失败仅告警。
            engine.persist_session();
            // 引擎退出（含 `hmp quit`）→ 置位 sticky 终止信号通知编排层收尾（spec §6；Finding 7）。
            let _ = engine.term_tx.send(true);
        });
        EngineHandle {
            command_tx: cmd_tx,
            state_rx,
            credential_ok,
            terminated: term_rx,
            caps_rx,
            queue_rx,
            library: None,
            sync_handle: None,
            comment: None,
            content: None,
            login: None,
            library_tx,
            library_rx,
        }
    }

    async fn run(&mut self) {
        // 启动即发布一次初始复合状态，保证订阅者拿到快照。
        self.publish();
        let mut events_rx = self.driver.subscribe_events();
        loop {
            tokio::select! {
                Some(req) = self.cmd_rx.recv() => {
                    match req {
                        Request::Quit => {
                            self.end_session("quit");
                            self.driver.shutdown();
                            break;
                        }
                        Request::Command(cmd) => self.handle_player_command(cmd).await,
                        Request::Play(src) => self.play_source(src, false).await,
                        Request::PlayList { ids, start } => self.play_list(ids, start).await,
                        Request::PlayNext(src) => self.play_source(src, true).await,
                        Request::QueueAppend(src) => {
                            match self.resolver.resolve_source_ids(&src).await {
                                Ok(stubs) => {
                                    self.cache_stubs(&stubs);
                                    let ids: Vec<TrackId> =
                                        stubs.into_iter().map(|s| s.id).collect();
                                    self.queue.append(ids);
                                    self.publish();
                                }
                                Err(e) => {
                                    self.last_error = Some(error_info(&e));
                                    self.publish();
                                }
                            }
                        }
                        Request::QueueRemove(i) => {
                            // 移除当前曲 = 播放接替曲（或空队列停止）。事务式（P1）：
                            // 先装载接替曲，成功后才关旧会话；装载失败回滚队列，
                            // 旧曲继续播放——不产生「队列已删、播放器仍播、会话已关」
                            // 的不一致中间态。
                            let was_current = self.queue.current_idx() == Some(i);
                            // 回滚快照须在 remove 之前保存（remove 后队列已变）。
                            let saved = self.queue.save_state();
                            if self.queue.remove(i) {
                                if was_current {
                                    // 装载前捕获旧会话与旧位置（listened_ms 用换曲时刻位置）。
                                    let old_session = self.session.clone();
                                    let old_position = self.state_rx.borrow().position;
                                    if let Some(id) = self.queue.current().cloned() {
                                        if self.load_and_play(id).await.is_ok() {
                                            // 装载成功：关闭命令前的旧会话。
                                            if let Some(old) = old_session {
                                                self.close_session(
                                                    &old,
                                                    "manual",
                                                    old_position.as_millis() as i64,
                                                );
                                            }
                                        } else {
                                            // 装载失败：回滚队列（被删曲目回到原位，
                                            // 旧曲继续播放）；last_error 已由 load_and_play 发布。
                                            self.queue.restore_state(saved);
                                            self.restore_phase_after_failure();
                                            self.publish(); // 回滚后重新发布（load_and_play 已发布中间态）
                                        }
                                    } else {
                                        // 空队列：确定性停止；阶段 → Idle。
                                        self.end_session("manual");
                                        self.last_error = None;
                                        self.driver.stop();
                                        self.enter_idle();
                                        self.publish();
                                    }
                                } else {
                                    self.publish();
                                }
                            }
                        }
                        Request::QueueClear { all } => {
                            if all {
                                // 清空并停止：播放器/会话/队列同步，不留「空队列仍在播」。
                                self.queue.clear();
                                self.end_session("stop");
                                self.last_error = None;
                                self.enter_idle();
                                self.driver.stop();
                            } else {
                                // 保留当前曲：清除待播曲目，播放/会话不受影响。
                                self.queue.clear_pending();
                            }
                            self.publish();
                        }
                        Request::QueuePlayAt(i) => {
                            self.queue_play_at(i).await;
                        }
                        Request::OpenUri(uri) => {
                            // MPRIS OpenUri：仅接受 file://（URL 解码后转本地播放）；其余 → 错误。
                            // to_file_path 在 Windows 上要求带盘符的 file path；无盘符的
                            // Unix 风格路径（file:///tmp/x.mp3，测试/跨端客户端）回退为
                            // 百分号解码后的 url.path() 原样本地路径。
                            let file_path = url::Url::parse(&uri)
                                .ok()
                                .filter(|u| u.scheme() == "file")
                                .and_then(|u| {
                                    u.to_file_path().ok().or_else(|| {
                                        percent_encoding::percent_decode_str(u.path())
                                            .decode_utf8()
                                            .ok()
                                            .map(|decoded| {
                                                std::path::PathBuf::from(decoded.into_owned())
                                            })
                                    })
                                });
                            match file_path {
                                Some(path) => {
                                    let src = PlayRequest::Local(TrackId::new(format!(
                                        "local:{}",
                                        path.display()
                                    )));
                                    self.play_source(src, false).await;
                                }
                                None => {
                                    self.last_error = Some(ErrorInfo {
                                        code: IpcErrorCode::Internal,
                                        message: format!("unsupported URI: {uri}"),
                                    });
                                    self.seq += 1;
                                    self.publish();
                                }
                            }
                        }
                        // 查询类由服务器直接读 state_rx 处理；引擎忽略（防御）。
                        _ => {}
                    }
                }
                _ = self.state_rx.changed() => {
                    self.publish();
                }
                ev = events_rx.recv() => {
                    match ev {
                        Ok(PlayerEvent::PlaybackEnded { load_gen }) => {
                            // 代际过滤（spec §7）：旧代 EOS 属已换下的曲目 → 忽略，
                            // 不触发换曲。同代 EOS 是真实曲尾（短曲立即结束也要续播）。
                            if load_gen != self.current_gen {
                                tracing::debug!(load_gen, current = self.current_gen, "忽略旧代 EOS");
                            } else {
                                self.on_ended().await;
                            }
                        }
                        Ok(PlayerEvent::Error { load_gen, .. }) => {
                            // 旧代错误事件属已换下的曲目 → 忽略（装载结果由 load_and_play 决定）。
                            if load_gen != self.current_gen {
                                tracing::debug!(load_gen, current = self.current_gen, "忽略旧代错误事件");
                            } else {
                                self.publish();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// 发布复合状态（playback 来自驱动 watch，queue 摘要 O(1)）。
    /// 完整队列快照仅在结构变化时发送到 `queue_tx`（position tick 不克隆）。
    /// 同时把精确的播放能力发布到 `caps_tx`（MPRIS 消费，Finding 9）。
    /// 尾部持久化：脏检查 + 节流写盘（里程碑 D）。
    fn publish(&mut self) {
        let caps = PlaybackCapabilities {
            can_go_next: self.queue.can_go_next(),
            can_go_previous: self.queue.can_go_previous(),
        };
        // user_volume 原值随状态发布（playback.volume 是 RG 补偿后的驱动值；
        // UI/MPRIS 展示与回设用原值，AUDIT §8.12）。
        let mut playback = self.state_rx.borrow().clone();
        playback.user_volume = self.user_volume;
        let state = DaemonState {
            playback,
            queue: self.queue.summary(),
            caps,
            seq: self.seq,
            last_error: self.last_error.clone(),
            replaygain_db: self.current_rg_db,
            phase: self.phase,
        };
        let _ = self.state_tx.send(state);
        if self.last_queue_rev != self.queue.revision() {
            self.last_queue_rev = self.queue.revision();
            let _ = self.queue_tx.send(self.queue.snapshot());
        }
        let _ = self.caps_tx.send(caps);
        self.persist_session();
    }

    /// 脏检查 + 节流写盘（写失败仅告警，不阻断播放）。
    /// 持久化**用户音量**（未含 RG 补偿）——补偿是装载期叠加量，恢复时
    /// 会随新曲重新计算；若持久化驱动侧补偿值，重启后 apply_gain 会二次
    /// 相乘（Review G2 WF1：音量漂移）。
    fn persist_session(&mut self) {
        let Some(path) = self.session_path.clone() else {
            return;
        };
        let playback = self.state_rx.borrow().clone();
        let rev = self.queue.revision();
        let volume = self.user_volume;
        // 当前曲 id + 位置（无当前曲 → None）。
        let position = self
            .queue
            .current()
            .map(|id| (id.clone(), playback.position.as_millis() as u64));
        let playing = playback.status == PlaybackStatus::Playing;
        let mut dirty = rev != self.saved.queue_rev || (volume - self.saved.volume).abs() > 1e-9;
        if position != self.saved.position {
            let now = std::time::Instant::now();
            let throttled = matches!(self.saved.last_write, Some(t) if now.duration_since(t) < self.persist_throttle);
            let must_flush = !playing || !throttled;
            dirty |= must_flush && position.is_some();
        }
        if playing != self.saved.playing {
            dirty = true; // 播放状态翻转（开始/暂停/停止）立即写
        }
        if !dirty {
            return;
        }
        let f = SessionFile {
            queue: self.queue.save_state(),
            // 用户音量（未补偿；恢复路径 restored_volume → user_volume）。
            volume: self.user_volume,
            position_ms: position.as_ref().map(|(_, ms)| *ms).unwrap_or(0),
        };
        match write_session_file(&path, &f) {
            Ok(()) => {
                self.saved.queue_rev = rev;
                self.saved.volume = volume;
                self.saved.playing = playing;
                self.saved.position = position;
                self.saved.last_write = Some(std::time::Instant::now());
            }
            Err(e) => tracing::warn!(%e, "failed to write session file"),
        }
    }

    async fn handle_player_command(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::Next => {
                self.navigate_next().await;
                self.seq += 1;
                self.publish();
            }
            PlayerCommand::Previous => {
                self.navigate_prev().await;
                self.seq += 1;
                self.publish();
            }
            PlayerCommand::SetLoopMode(m) => {
                self.queue.set_loop_mode(m);
                self.driver.command(PlayerCommand::SetLoopMode(m));
                self.publish();
            }
            PlayerCommand::SetShuffle(b) => {
                self.queue.set_shuffle(b);
                self.driver.command(PlayerCommand::SetShuffle(b));
                self.publish();
            }
            PlayerCommand::Stop => {
                self.end_session("stop");
                self.driver.command(PlayerCommand::Stop);
                self.phase = hmp_core::EnginePhase::Idle;
                self.publish();
            }
            PlayerCommand::LoadAndPlay(_) => {
                // 队列场景不使用（CLI/桌面按 id 走 Play 请求）；忽略。
            }
            PlayerCommand::SetVolume(v) => {
                // G2：用户音量与 RG 补偿分离——记录用户音量，驱动收到叠加补偿后的值
                // （换曲时 apply_gain 按新曲增益重算）。
                self.user_volume = v;
                self.driver
                    .command(PlayerCommand::SetVolume(v * self.rg_factor));
            }
            other => self.driver.command(other), // Play/Pause/Stop/Seek/TogglePlay 直通驱动
        }
    }

    async fn navigate_next(&mut self) {
        // 先裁决再换会话（P1：队列无可跳目标时不得先关掉当前会话）。
        let saved = self.queue.save_state();
        let Some(id) = self.queue.skip_next() else {
            return;
        };
        // 装载前捕获旧会话与旧位置（listened_ms 用换曲时刻位置，非新曲 ~0）。
        let old_session = self.session.clone();
        let old_position = self.state_rx.borrow().position;
        if self.load_and_play(id).await.is_ok() {
            // 装载成功才切换会话：关闭命令前打开的会话。
            if let Some(old) = old_session {
                self.close_session(&old, "next", old_position.as_millis() as i64);
            }
        } else {
            // 装载失败：回滚队列位置（原曲继续播放，状态一致）。
            self.queue.restore_state(saved);
            self.restore_phase_after_failure();
        }
    }

    /// 队列跳播（AUDIT §8.8）：跳到 0 基位置曲目播放，**不替换队列**。
    /// 事务式（与 navigate_next 同模式）：先装载成功再提交队列游标与会话；
    /// 失败回滚，旧曲继续。越界/已是当前曲 → 不动作（发布错误）。
    async fn queue_play_at(&mut self, index: usize) {
        let len = self.queue.summary().len;
        if index >= len {
            self.last_error = Some(ErrorInfo {
                code: IpcErrorCode::Internal,
                message: format!("queue play-at index {index} out of range (len {len})"),
            });
            self.seq += 1;
            self.publish();
            return;
        }
        if self.queue.current_idx() == Some(index) {
            // 点当前曲 = 确保播放（对齐队列抽屉「点行即播」直觉）。
            self.driver.play();
            self.seq += 1;
            self.publish();
            return;
        }
        let saved = self.queue.save_state();
        self.queue.set_current(index);
        let Some(id) = self.queue.current().cloned() else {
            return;
        };
        let old_session = self.session.clone();
        let old_position = self.state_rx.borrow().position;
        if self.load_and_play(id).await.is_ok() {
            if let Some(old) = old_session {
                self.close_session(&old, "manual", old_position.as_millis() as i64);
            }
        } else {
            // 装载失败：回滚游标（原曲继续播放，状态一致）。
            self.queue.restore_state(saved);
            self.restore_phase_after_failure();
        }
        self.seq += 1;
        self.publish();
    }

    async fn navigate_prev(&mut self) {
        // 曲首语义（里程碑 G，审计第 6 步）：当前位置 > 3s → 只回曲首，
        // 不换曲（与会话记录/队列无交互；与 MPRIS Seek 行为一致）。
        if self.state_rx.borrow().position > std::time::Duration::from_secs(3) {
            self.driver
                .command(PlayerCommand::Seek(std::time::Duration::ZERO));
            return;
        }
        let saved = self.queue.save_state();
        let Some(id) = self.queue.prev_track() else {
            return;
        };
        let old_session = self.session.clone();
        let old_position = self.state_rx.borrow().position;
        if self.load_and_play(id).await.is_ok() {
            if let Some(old) = old_session {
                self.close_session(&old, "previous", old_position.as_millis() as i64);
            }
        } else {
            self.queue.restore_state(saved);
            self.restore_phase_after_failure();
        }
    }

    /// Play / PlayNext：解析源 → 替换/插入队列 → 加载当前。
    ///
    /// seq 在**命令完成后**（解析+装载结束，无论成败）推进并发布：
    /// CLI 以 seq 前进作为「本命令结果已可见」的边界。中间发布保持旧 seq，
    /// 避免 CLI 在解析/装载窗口误判（Bug 1：Empty 误报；Bug 2：旧曲目确认）。
    ///
    /// **事务式换曲**（P1）：先装载（队列与会话不动），装载成功后才提交
    /// 队列变更与会话切换；装载失败则保持旧队列/旧会话/旧曲继续播放，
    /// 仅发布错误——CLI 不再把旧曲目当成新请求成功。
    async fn play_source(&mut self, src: PlayRequest, playnext: bool) {
        self.phase = hmp_core::EnginePhase::Resolving;
        let stubs = match self.resolver.resolve_source_ids(&src).await {
            Ok(stubs) => stubs,
            Err(e) => {
                // 解析失败 → 发布错误详情（Finding 2）+ 推进命令代际；
                // 阶段恢复：旧曲仍在播 → Playing，否则 Idle。
                self.last_error = Some(error_info(&e));
                self.restore_phase_after_failure();
                self.seq += 1;
                self.publish();
                return;
            }
        };
        if stubs.is_empty() {
            // 空源是确定性失败：携带错误，CLI 不用等到超时。
            self.last_error = Some(ErrorInfo {
                code: IpcErrorCode::Internal,
                message: "source resolved to no tracks".into(),
            });
            self.restore_phase_after_failure();
            self.seq += 1;
            self.publish();
            return;
        }
        // 列表元数据批量缓存进媒体库（投影层查询用；库不可用不阻断播放）。
        self.cache_stubs(&stubs);
        let ids: Vec<TrackId> = stubs.iter().map(|s| s.id.clone()).collect();
        // 装载前捕获旧会话与旧位置（listened_ms 用换曲时刻位置，非新曲 ~0）。
        let old_session = self.session.clone();
        let old_position = self.state_rx.borrow().position;
        // 提交后预解析的锚点（ids 随后被 replace/insert 移走）。
        let first_id = ids[0].clone();
        match self.load_and_play(ids[0].clone()).await {
            Ok(()) => {
                // 提交：关闭命令前打开的旧会话。
                if let Some(old) = old_session {
                    self.close_session(&old, "manual", old_position.as_millis() as i64);
                }
                if playnext {
                    // 整片插入当前曲之后（多曲目；空队列按 replace 建队）。
                    if let Some(at) = self.queue.insert_after_current(ids) {
                        self.queue.set_current(at); // 当前曲定位到插入的首曲（开始播放它）
                    }
                } else {
                    let first = ids[0].clone();
                    self.queue.replace(ids, 0);
                    // 会话恢复续播：首次 Play 且装载曲目 == 恢复的 current
                    // → Seek 到保存位置；随后清除恢复上下文（换曲即弃）。
                    if let Some(r) = self.restored.take() {
                        if first == r.current {
                            self.driver.command(PlayerCommand::Seek(
                                std::time::Duration::from_millis(r.position_ms),
                            ));
                        }
                    }
                }
                self.seq += 1;
                self.publish();
                // G2：队列已提交（replace/insert）→ 预解析下一首（装载时队列为空
                // 无法预判，故在提交后补一次；navigate/EOS 路径由 load_and_play 内触发）。
                self.schedule_preload(&first_id);
            }
            Err(e) => {
                // 装载失败：队列/会话/播放均保持原状，仅发布错误（P1）；
                // 阶段恢复：旧曲仍在播 → Playing。
                self.last_error = Some(error_info(&e));
                self.restore_phase_after_failure();
                self.seq += 1;
                self.publish();
            }
        }
    }

    /// `Request::PlayList`（GUI 列表入口）：显式曲目列表整表替换 + 起播下标，
    /// 与 [`play_source`] 同一事务式流程——起播曲装载成功才提交
    /// `queue.replace(ids, start)`；装载/解析失败保持旧队列/旧曲，仅发布错误。
    ///
    /// stub 解析走两路：id 已在媒体库（GUI 列表本身来自库投影）→ 批量读库
    /// 构建（免逐文件 read_meta，大列表「全部播放」不卡引擎命令循环）；
    /// 未命中才走解析器（本地读标签、QQ 落 id stub——库行标题不为 mid 所覆写，
    /// 显示层元数据由客户端 overlay 兜底）。seq 同 play_source：命令完成后推进。
    async fn play_list(&mut self, ids: Vec<TrackId>, start: usize) {
        if ids.is_empty() {
            self.last_error = Some(ErrorInfo {
                code: IpcErrorCode::Internal,
                message: "play list is empty".into(),
            });
            self.restore_phase_after_failure();
            self.seq += 1;
            self.publish();
            return;
        }
        self.phase = hmp_core::EnginePhase::Resolving;
        let stubs = match self.resolve_id_stubs(&ids).await {
            Ok(stubs) => stubs,
            Err(e) => {
                self.last_error = Some(error_info(&e));
                self.restore_phase_after_failure();
                self.seq += 1;
                self.publish();
                return;
            }
        };
        self.cache_stubs(&stubs);
        let start = start.min(ids.len() - 1);
        let first_id = ids[start].clone();
        // 装载前捕获旧会话与旧位置（与 play_source 同模式）。
        let old_session = self.session.clone();
        let old_position = self.state_rx.borrow().position;
        match self.load_and_play(first_id.clone()).await {
            Ok(()) => {
                if let Some(old) = old_session {
                    self.close_session(&old, "manual", old_position.as_millis() as i64);
                }
                self.queue.replace(ids, start);
                // 会话恢复续播（与 play_source 同语义）：起播曲命中恢复曲 → seek。
                if let Some(r) = self.restored.take() {
                    if first_id == r.current {
                        self.driver
                            .command(PlayerCommand::Seek(std::time::Duration::from_millis(
                                r.position_ms,
                            )));
                    }
                }
                self.seq += 1;
                self.publish();
                self.schedule_preload(&first_id);
            }
            Err(e) => {
                self.last_error = Some(error_info(&e));
                self.restore_phase_after_failure();
                self.seq += 1;
                self.publish();
            }
        }
    }

    /// 列表 id → stub：库内 id 批量投影（快路径），未命中走解析器逐个补。
    /// 解析器失败（如 QQ 未登录）整体失败——半截列表不提交队列。
    async fn resolve_id_stubs(
        &self,
        ids: &[TrackId],
    ) -> Result<Vec<hmp_core::TrackStub>, EngineError> {
        let mut known: std::collections::HashMap<String, hmp_core::TrackStub> =
            std::collections::HashMap::new();
        if let Some(library) = &self.library {
            let mut qq_keys = Vec::new();
            let mut local_keys = Vec::new();
            for id in ids {
                if hmp_core::TrackProvider::from_id(id.as_ref()) == hmp_core::TrackProvider::Local {
                    local_keys.push(id.to_string());
                } else {
                    qq_keys.push(id.to_string());
                }
            }
            let mut lib = library.lock().unwrap();
            for source_meta in [("qq", &qq_keys), ("local", &local_keys)] {
                let (source, keys) = source_meta;
                for meta in lib.track_meta_batch(source, keys).unwrap_or_default() {
                    known.insert(
                        meta.source_key.clone(),
                        hmp_core::TrackStub {
                            id: TrackId::new(meta.source_key.clone()),
                            title: meta.title,
                            artists: meta.artist.into_iter().collect(),
                            album: meta.album,
                            duration_ms: meta.duration_ms.map(|ms| ms.clamp(0, u32::MAX as i64)),
                        },
                    );
                }
            }
        }
        let mut stubs = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(stub) = known.get(id.as_ref()) {
                stubs.push(stub.clone());
                continue;
            }
            let resolved = self
                .resolver
                .resolve_source_ids(&PlayRequest::Track(id.clone()))
                .await?;
            stubs.extend(resolved);
        }
        Ok(stubs)
    }

    async fn on_ended(&mut self) {
        self.end_session("ended");
        let saved = self.queue.save_state();
        if let Some(id) = self.queue.advance_on_eos() {
            self.publish();
            if self.load_and_play(id).await.is_err() {
                // 续播失败：回滚队列位置（已播完的曲目停在当前位置）。
                self.queue.restore_state(saved);
                self.restore_phase_after_failure();
            }
            self.publish();
        } else {
            // 无续播：阶段 → Idle，RG 增益随曲目清空（Review 打磨：
            // 避免 status 在曲目为空时残留上一曲的增益）。
            self.enter_idle();
            self.publish();
        }
    }

    /// 媒体库：upsert 曲目并开启播放会话（B4 会话粒度：INSERT play_events）。
    /// 库不可用/写失败不阻断播放（仅 warn 级）。
    /// 媒体库：upsert 曲目并开启播放会话（B4 会话粒度：INSERT play_events 返回
    /// event id）。每次播放动作独立会话（同曲重播也新建——listened_ms 各自记录）。
    fn start_session(&mut self, track: &hmp_core::Track) {
        let Some(library) = &self.library else {
            return;
        };
        let mut library = library.lock().unwrap();
        let row = track_row(track);
        match library.upsert_track(&row) {
            Ok(track_id) => match library.record_play_start(track_id, now_unix()) {
                Ok(event_id) => {
                    self.session = Some(PlaybackSession { track_id, event_id });
                }
                Err(e) => tracing::warn!(%e, "failed to start library play session"),
            },
            Err(e) => tracing::warn!(%e, "library upsert failed"),
        }
    }

    /// 媒体库：结束当前播放会话（按 event id 精确闭合 + 播放次数）。
    /// 收听时长 = 当前播放位置（位置无时长上限时原样记录）。
    fn end_session(&mut self, reason: &'static str) {
        if let Some(s) = self.session.take() {
            let listened_ms = self.state_rx.borrow().position.as_millis() as i64;
            self.close_session(&s, reason, listened_ms);
        }
    }

    /// 按事件 id 关闭播放会话（事务提交路径用：换曲前捕获的旧位置作 listened_ms）。
    fn close_session(&self, s: &PlaybackSession, reason: &'static str, listened_ms: i64) {
        let Some(library) = &self.library else {
            return;
        };
        let mut library = library.lock().unwrap();
        let end = hmp_storage::PlayEnd {
            track_id: s.track_id,
            ended_at: now_unix(),
            listened_ms,
            reason,
        };
        if let Err(e) = library.record_play_end(s.event_id, &end) {
            tracing::warn!(%e, "failed to close library play session");
        }
    }

    /// 列表解析元数据批量缓存进媒体库（stub → tracks 行，单事务；投影层查询用）。
    /// 库不可用/写失败仅 warn，不阻断播放（与 `start_session` 同一原则）。
    fn cache_stubs(&self, stubs: &[hmp_core::TrackStub]) {
        let Some(library) = &self.library else {
            return;
        };
        let rows: Vec<hmp_storage::TrackRow> = stubs.iter().map(stub_row).collect();
        let mut library = library.lock().unwrap();
        if let Err(e) = library.upsert_tracks_batch(&rows) {
            tracing::warn!(%e, "library batch cache failed");
        }
    }

    /// 解析 + 解密 + 加载 + 播放。装载失败返回错误（调用方决定回滚/保持）。
    async fn load_and_play(&mut self, id: TrackId) -> Result<(), EngineError> {
        // 成功路径：清除旧错误（Finding 2）；进入装载阶段（spec §7）并发布
        // （订阅者可见 Loading 中间态；seq 未动，CLI 确认逻辑不受影响）。
        self.last_error = None;
        self.phase = hmp_core::EnginePhase::Loading;
        self.publish();
        // 预解析缓存（G2）：代际匹配 + 曲目 id 相符 → 直接消费（跳过网络解析）。
        // 注意：消费条件**不含队列 revision**——skip_next/advance_on_eos 本身会
        // bump revision，若纳入则缓存永远无法命中；id 校验已保证不消费错曲。
        let cached = {
            let mut slot = self.preload_slot.lock().await;
            match slot.as_ref() {
                Some(s) if s.key.1 == self.current_gen && s.id == id => slot.take().map(|s| s.res),
                _ => None,
            }
        };
        let res = match cached {
            Some(res) => res,
            None => match self.resolver.resolve_track(&id).await {
                Ok(res) => res,
                Err(e) => {
                    tracing::error!(%e, "resolve failed: {id}");
                    // 队列位置保持；错误详情进入复合状态（Finding 2）；阶段 → Failed。
                    self.last_error = Some(error_info(&e));
                    self.phase = hmp_core::EnginePhase::Failed;
                    self.publish();
                    return Err(e);
                }
            },
        };
        // 捕获上一装载与旧位置（回滚与历史用；此时尚未触碰任何状态）。
        let prev = self.last_load.clone();
        let prev_position = self.state_rx.borrow().position;
        let uri = res.uri.clone();
        let quality = res.quality;
        let expected = res.track.id.clone();
        self.current_gen += 1;
        let load_gen = self.current_gen;
        self.driver.load(LoadRequest {
            track: res.track.clone(),
            uri,
            quality: quality.clone(),
            load_gen,
        });
        self.driver.play();
        // 等待驱动应用装载（真实驱动为异步管道）：完成前发布的复合状态
        // 不得携带旧曲目（Bug 2：play-next 后显示旧曲）。超时/通道断开 →
        // 失败路径（调用方回滚队列、保留旧曲；不创建播放历史）。
        if let Err(e) = self.wait_current_applied(&expected, load_gen).await {
            // 未确认装载：新解密代理此刻释放；旧 active_media 保持。
            drop(res.media);
            if let Some(p) = prev {
                // 复原代际：回滚后旧曲重新成为当前代（driver loaded_gen 已
                // 重载为 prev.load_gen），其 EOS/Error 不得再被误判为旧代
                // 忽略（否则播完不续播、会话不闭合）。失败装载 b 的迟到
                // 事件 gen=N+1 恰好被过滤，语义正确。
                self.current_gen = p.load_gen;
                self.rollback_load(p, prev_position).await;
            }
            self.last_error = Some(error_info(&e));
            self.phase = hmp_core::EnginePhase::Failed;
            self.publish();
            return Err(e);
        }
        // ACK 成功才提交：替换 active_media（旧代理此刻才释放）、
        // 记录装载（回滚用）、进入播放阶段、开启播放会话。
        // G2：ReplayGain 补偿（用户音量 × 当前曲增益；仅成功路径更新 rg_factor）。
        self.apply_gain(res.replaygain_db);
        self.active_media = res.media;
        self.last_load = Some(AppliedLoad {
            track: res.track.clone(),
            uri: res.uri,
            quality,
            load_gen,
        });
        self.phase = hmp_core::EnginePhase::Playing;
        // 媒体库：upsert 曲目 + 开启播放会话（B4）。
        self.start_session(&res.track);
        // G2：预解析队列下一首（仅成功路径；队列无下一首则跳过）。
        self.schedule_preload(&res.track.id);
        self.publish();
        Ok(())
    }

    /// G2：应用当前曲的 ReplayGain 补偿。`factor = 10^(dB/20)`，clamp 到
    /// [0.25, 4.0]（±12dB，防异常标签）；配置 `[audio] replaygain=false`
    /// 时恒为 1.0。驱动音量 = 用户音量 × 因子（用户调音量不丢补偿）。
    fn apply_gain(&mut self, replaygain_db: Option<f64>) {
        // 打磨：记录标签 dB 原值（随 DaemonState 发布供 CLI 展示）。
        self.current_rg_db = replaygain_db;
        let enabled = hmp_storage::Config::load().audio.replaygain;
        self.rg_factor = if enabled {
            match replaygain_db {
                Some(db) => (10f64).powf(db / 20.0).clamp(0.25, 4.0),
                None => 1.0,
            }
        } else {
            1.0
        };
        self.driver.set_volume(self.user_volume * self.rg_factor);
    }

    /// G2：后台预解析队列下一首（gapless 加速曲间切换）。
    /// 仅当装载曲目 == 当前队列 current 时触发（navigate/EOS 已先移动
    /// cursor；play_source 装载时队列尚未提交 → 跳过，由提交后补触发）。
    /// 指纹 `(队列 revision, 装载代际)` 仅用于**写槽防乱序**（旧任务不得
    /// 覆盖新任务结果，字典序比较）；失败静默（不影响播放）。
    fn schedule_preload(&self, loaded: &TrackId) {
        // 打磨：Repeat One（Track）EOS 重播当前曲——预解析当前曲冗余
        //（重复 resolve + 重复解密代理），跳过；手动 Next 不受影响
        //（用户主动操作走正常 resolve 延迟可接受）。
        if self.queue.loop_mode() == hmp_core::LoopMode::Track {
            return;
        }
        if self.queue.current() != Some(loaded) {
            return;
        }
        let Some(next) = self.queue.peek_next() else {
            return;
        };
        let key = (self.queue.revision(), self.current_gen);
        let resolver = self.resolver.clone();
        let slot = self.preload_slot.clone();
        tokio::spawn(async move {
            match resolver.resolve_track(&next).await {
                Ok(res) => {
                    let mut g = slot.lock().await;
                    // 乱序保护：仅当槽为空或槽的 key 不新于本次（<=）才写入。
                    let stale_or_same = match g.as_ref() {
                        None => true,
                        Some(s) => (s.key.0, s.key.1) <= (key.0, key.1),
                    };
                    if stale_or_same {
                        *g = Some(PreloadSlot { key, id: next, res });
                    }
                }
                Err(e) => tracing::debug!(%e, "预解析失败（不影响播放）"),
            }
        });
    }

    /// 进入 Idle（队列已清空/播完）：同步清 RG 残留——`current_rg_db` 不再
    /// 随 status 显示已消失曲目的增益，`rg_factor` 归一避免此后 SetVolume
    /// 被旧曲增益污染（与自然播完路径同一语义）。
    fn enter_idle(&mut self) {
        self.current_rg_db = None;
        self.rg_factor = 1.0;
        self.phase = hmp_core::EnginePhase::Idle;
    }

    /// 装载/解析失败后的阶段恢复：旧曲仍在播 → Playing，否则 Idle。
    /// 回滚调用方（navigate/QueueRemove/on_ended）在 restore 后调用。
    fn restore_phase_after_failure(&mut self) {
        let playing = self.state_rx.borrow().current.is_some();
        self.phase = if playing {
            hmp_core::EnginePhase::Playing
        } else {
            hmp_core::EnginePhase::Idle
        };
    }

    /// 等待驱动把 current 更新为 `expected`（同步应用的驱动立即返回；
    /// 异步音频驱动等待其装载任务发布）。
    /// 超时（`load_timeout`，默认 5s）→ `Timeout`：调用方按装载失败处理
    /// （回滚队列、旧曲继续），不得把未确认的装载当成功提交
    /// （此前仅 warn 后继续置 Playing/建历史）。
    /// 两条失败出口：驱动 `Error` 事件（同代，打开/解码失败——**即时**
    /// 返回，「点了没反应」窗口从 5s 收到立即）与超时兜底（驱动静默
    /// 卡死）。完成前发布的复合状态不得携带旧曲目（Bug 2：play-next
    /// 后显示旧曲）。
    async fn wait_current_applied(
        &mut self,
        expected: &TrackId,
        load_gen: u64,
    ) -> Result<(), EngineError> {
        let deadline = tokio::time::Instant::now() + self.load_timeout;
        // 独立事件订阅：只看本次订阅之后的事件（订阅前已发 error 的微小
        // 竞口由超时兜底），同代过滤防旧曲错误误伤。
        let mut events = self.driver.subscribe_events();
        loop {
            {
                let cur = self.state_rx.borrow();
                if cur.current.as_ref().map(|t| &t.id) == Some(expected) {
                    return Ok(());
                }
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(EngineError::Timeout);
            }
            tokio::select! {
                // changed() 在无新状态时挂起：必须用剩余时间兜底，否则驱动
                // 不发布任何状态（如装载失败静默）时永不超时。
                changed = self.state_rx.changed() => {
                    if changed.is_err() {
                        return Err(EngineError::Internal("state channel closed".into()));
                    }
                }
                ev = events.recv() => {
                    match ev {
                        Ok(PlayerEvent::Error { load_gen: ev_gen, error }) if ev_gen == load_gen => {
                            return Err(EngineError::Internal(format!(
                                "driver load error: {error}"
                            )));
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return Err(EngineError::Internal("event channel closed".into()));
                        }
                    }
                }
                _ = tokio::time::sleep(remaining) => {
                    return Err(EngineError::Timeout);
                }
            }
        }
    }

    /// 装载失败后的尽力回滚：重载上一首并恢复到其位置。
    /// 沿用原代际（调用方已在失败路径把 current_gen 复原为 prev.load_gen，
    /// 故回滚后旧曲 EOS/Error 仍属当前代，不会被过滤）；未确认仅 warn。
    ///
    /// 回滚上一条已应用装载，并恢复此前的播放位置。
    async fn rollback_load(&mut self, prev: AppliedLoad, position: std::time::Duration) {
        let id = prev.track.id.clone();
        self.driver.load(LoadRequest {
            track: prev.track.clone(),
            uri: prev.uri,
            quality: prev.quality,
            load_gen: prev.load_gen,
        });
        if self.wait_current_applied(&id, prev.load_gen).await.is_ok() {
            self.driver.seek(position);
            self.driver.play();
        } else {
            tracing::warn!("rollback load not confirmed (previous track may not be restored)");
        }
    }
}

/// 当前 unix 时间戳（秒）。
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `hmp_core::Track` → 媒体库行（窄投影：只存稳定身份与元数据，不存播放 URL）。
/// 按 provider 写 source（P1：本地曲目不得写 `qq`，否则同一文件在
/// tracks 中生成两条记录——local_files 连一条、播放历史连另一条）。
fn track_row(t: &hmp_core::Track) -> hmp_storage::TrackRow {
    let source = if hmp_core::TrackProvider::from_id(&t.id.0) == hmp_core::TrackProvider::Local {
        "local"
    } else {
        "qq"
    };
    hmp_storage::TrackRow {
        source,
        source_key: t.id.0.clone(),
        title: t.title.clone(),
        album: t.album.as_ref().map(|a| a.name.clone()),
        artist: {
            let names = t.artist_names();
            (!names.is_empty()).then_some(names)
        },
        duration_ms: t.duration.map(|d| d.as_millis() as i64),
        cover_uri: t.cover.as_ref().map(|c| c.url.clone()),
        qq_song_id: None, // 播放路径无 numeric id；列表解析缓存（stub_row）时写入
        ..Default::default()
    }
}

/// stub → 媒体库行（批量缓存；source 规则与 `track_row` 一致）。
fn stub_row(s: &hmp_core::TrackStub) -> hmp_storage::TrackRow {
    let source = if hmp_core::TrackProvider::from_id(&s.id.0) == hmp_core::TrackProvider::Local {
        "local"
    } else {
        "qq"
    };
    hmp_storage::TrackRow {
        source,
        source_key: s.id.to_string(),
        title: s.title.clone(),
        album: s.album.clone(),
        artist: (!s.artists.is_empty()).then(|| s.artists.join(", ")),
        duration_ms: s.duration_ms,
        cover_uri: None,
        qq_song_id: None, // TrackStub 不含 numeric id；后续由列表解析补全
        ..Default::default()
    }
}

/// 引擎错误 → IPC 错误码 + 人类可读消息（Finding 2）。
fn error_info(e: &EngineError) -> ErrorInfo {
    let code = match e {
        EngineError::NotLoggedIn => IpcErrorCode::NotLoggedIn,
        EngineError::TrackNotFound => IpcErrorCode::TrackNotFound,
        EngineError::PlaylistNotFound(_) => IpcErrorCode::PlaylistNotFound,
        EngineError::QualityUnavailable(_) => IpcErrorCode::QualityUnavailable,
        EngineError::Timeout => IpcErrorCode::Internal,
        EngineError::Internal(_) => IpcErrorCode::Internal,
    };
    ErrorInfo {
        code,
        message: e.to_string(),
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
