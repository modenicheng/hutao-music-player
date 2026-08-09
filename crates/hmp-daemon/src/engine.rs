//! 播放引擎：命令循环 + 队列裁决 + 自动续播 + 复合状态发布（spec §4.2 `daemon.rs`）。
//!
//! 单一命令通道：所有输入适配器（socket 服务器 / tray / MPRIS）把
//! [`Request`] 发进 [`EngineHandle::command_tx`]，由引擎串行处理；
//! 单一状态出口：`watch<DaemonState>`。Next/Previous 由引擎拦截做队列
//! 导航（PlayerCore 忽略这两个命令，见 hmp-player-gst core.rs）。

use std::sync::Arc;

use hmp_core::{
    DaemonState, ErrorInfo, IpcErrorCode, PlayRequest, PlaybackCapabilities, PlaybackState,
    PlayerCommand, QueueSnapshot, Request, TrackId,
};
use hmp_player_gst::PlayerEvent;
use tokio::sync::{mpsc, watch};

use crate::player::{EngineError, PlaybackDriver, SourceResolver};

/// 最近一次成功装载的完整信息（失败回滚用）。
#[derive(Clone)]
struct AppliedLoad {
    track: hmp_core::Track,
    uri: String,
    quality: hmp_core::AudioQuality,
    load_gen: u64,
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
    /// 等待驱动应用装载的超时（测试注入短超时）。
    load_timeout: std::time::Duration,
    /// 完整队列快照（仅结构变化时发送；position tick 不触发——O(1) publish）。
    queue_tx: watch::Sender<QueueSnapshot>,
    /// 上次发布的队列版本（避免重复发送；publish 为 &self，用内部可变性）。
    last_queue_rev: std::cell::Cell<u64>,
}

impl PlaybackEngine {
    /// 启动引擎（spawn 主循环任务），返回句柄。
    pub fn start(
        driver: Arc<dyn PlaybackDriver>,
        resolver: Arc<dyn SourceResolver>,
        credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> EngineHandle {
        Self::start_with_library(driver, resolver, credential_ok, None)
    }

    /// 启动引擎并挂载媒体库（B4：播放会话写库）。
    pub fn start_with_library(
        driver: Arc<dyn PlaybackDriver>,
        resolver: Arc<dyn SourceResolver>,
        credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
        library: Option<std::sync::Arc<std::sync::Mutex<hmp_storage::LibraryDb>>>,
    ) -> EngineHandle {
        Self::start_with_options(
            driver,
            resolver,
            credential_ok,
            library,
            std::time::Duration::from_secs(5),
        )
    }

    /// 带装载超时注入的启动（测试用短超时驱动失败路径）。
    pub fn start_with_options(
        driver: Arc<dyn PlaybackDriver>,
        resolver: Arc<dyn SourceResolver>,
        credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
        library: Option<std::sync::Arc<std::sync::Mutex<hmp_storage::LibraryDb>>>,
        load_timeout: std::time::Duration,
    ) -> EngineHandle {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (state_tx, state_rx) = watch::channel(DaemonState::default());
        let playback_rx = driver.subscribe_state();
        let (caps_tx, caps_rx) = watch::channel(PlaybackCapabilities::default());
        let (queue_tx, queue_rx) = watch::channel(QueueSnapshot::default());
        // sticky 终止信号：晚到的接收者立即可见（watch 保留当前值，Finding 7）。
        let (term_tx, term_rx) = watch::channel(false);
        let mut engine = Self {
            driver,
            resolver,
            queue: hmp_core::QueueCore::new(),
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
            load_timeout,
            queue_tx,
            last_queue_rev: std::cell::Cell::new(0),
        };
        tokio::spawn(async move {
            engine.run().await;
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
                            let was_current = self.queue.snapshot().current == Some(i);
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
                                        self.phase = hmp_core::EnginePhase::Idle;
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
                                self.phase = hmp_core::EnginePhase::Idle;
                                self.driver.stop();
                            } else {
                                // 保留当前曲：清除待播曲目，播放/会话不受影响。
                                self.queue.clear_pending();
                            }
                            self.publish();
                        }
                        Request::OpenUri(uri) => {
                            // MPRIS OpenUri：仅接受 file://（URL 解码后转本地播放）；其余 → 错误。
                            match url::Url::parse(&uri)
                                .ok()
                                .and_then(|u| u.to_file_path().ok())
                            {
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
                                        message: format!("不支持的 URI: {uri}"),
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

    /// 发布复合状态（playback 来自驱动 watch，queue 来自队列核心）。
    /// 同时把精确的播放能力发布到 `caps_tx`（MPRIS 消费，Finding 9）。
    /// 发布复合状态（playback 来自驱动 watch，queue 摘要 O(1)）。
    /// 完整队列快照仅在结构变化时发送到 `queue_tx`（position tick 不克隆）。
    /// 同时把精确的播放能力发布到 `caps_tx`（MPRIS 消费，Finding 9）。
    fn publish(&self) {
        let caps = PlaybackCapabilities {
            can_go_next: self.queue.can_go_next(),
            can_go_previous: self.queue.can_go_previous(),
        };
        let state = DaemonState {
            playback: self.state_rx.borrow().clone(),
            queue: self.queue.summary(),
            caps,
            seq: self.seq,
            last_error: self.last_error.clone(),
            phase: self.phase,
        };
        let _ = self.state_tx.send(state);
        if self.last_queue_rev.get() != self.queue.revision() {
            self.last_queue_rev.set(self.queue.revision());
            let _ = self.queue_tx.send(self.queue.snapshot());
        }
        let _ = self.caps_tx.send(caps);
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
            other => self.driver.command(other), // Play/Pause/Stop/Seek/Volume/TogglePlay 直通驱动
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

    async fn navigate_prev(&mut self) {
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
                message: "源解析结果为空，无曲目可播放".into(),
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
                    self.queue.replace(ids, 0);
                }
                self.seq += 1;
                self.publish();
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
            // 无续播：阶段 → Idle。
            self.phase = hmp_core::EnginePhase::Idle;
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
                Err(e) => tracing::warn!(%e, "媒体库会话开启失败"),
            },
            Err(e) => tracing::warn!(%e, "媒体库 upsert 失败"),
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
            tracing::warn!(%e, "媒体库会话结束回写失败");
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
            tracing::warn!(%e, "媒体库批量缓存失败");
        }
    }

    /// 解析 + 解密 + 加载 + 播放。装载失败返回错误（调用方决定回滚/保持）。
    async fn load_and_play(&mut self, id: TrackId) -> Result<(), EngineError> {
        // 成功路径：清除旧错误（Finding 2）；进入装载阶段（spec §7）并发布
        // （订阅者可见 Loading 中间态；seq 未动，CLI 确认逻辑不受影响）。
        self.last_error = None;
        self.phase = hmp_core::EnginePhase::Loading;
        self.publish();
        match self.resolver.resolve_track(&id).await {
            Ok(res) => {
                // 捕获上一装载与旧位置（回滚与历史用；此时尚未触碰任何状态）。
                let prev = self.last_load.clone();
                let prev_position = self.state_rx.borrow().position;
                let uri = res.uri.clone();
                let quality = res.quality;
                let expected = res.track.id.clone();
                self.current_gen += 1;
                let load_gen = self.current_gen;
                self.driver.load(hmp_player_gst::LoadRequest {
                    track: res.track.clone(),
                    uri,
                    quality: quality.clone(),
                    load_gen,
                });
                self.driver.play();
                // 等待驱动应用装载（真实驱动为异步管道）：完成前发布的复合状态
                // 不得携带旧曲目（Bug 2：play-next 后显示旧曲）。超时/通道断开 →
                // 失败路径（调用方回滚队列、保留旧曲；不创建播放历史）。
                if let Err(e) = self.wait_current_applied(&expected).await {
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
                self.publish();
                Ok(())
            }
            Err(e) => {
                tracing::error!(%e, "解析失败: {id}");
                // 队列位置保持；错误详情进入复合状态（Finding 2）；阶段 → Failed。
                self.last_error = Some(error_info(&e));
                self.phase = hmp_core::EnginePhase::Failed;
                self.publish();
                Err(e)
            }
        }
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
    /// 异步管道（真实 GStreamer）等待其装载臂发布）。
    /// 超时（`load_timeout`，默认 5s）→ `Timeout`：调用方按装载失败处理
    /// （回滚队列、旧曲继续），不得把未确认的装载当成功提交
    /// （此前仅 warn 后继续置 Playing/建历史）。
    async fn wait_current_applied(&mut self, expected: &TrackId) -> Result<(), EngineError> {
        let deadline = tokio::time::Instant::now() + self.load_timeout;
        loop {
            {
                let cur = self.state_rx.borrow();
                if cur.current.as_ref().map(|t| &t.id) == Some(expected) {
                    return Ok(());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(EngineError::Timeout);
            }
            // changed() 在无新状态时挂起：必须用 timeout 包裹，否则驱动不发布
            // 任何状态（如装载失败静默）时永不超时——失败装载无法被识别。
            match tokio::time::timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                self.state_rx.changed(),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err(EngineError::Internal("状态通道已关闭".into())),
                Err(_elapsed) => return Err(EngineError::Timeout),
            }
        }
    }

    /// 装载失败后的尽力回滚：重载上一首并恢复到其位置。
    /// 沿用原代际（调用方已在失败路径把 current_gen 复原为 prev.load_gen，
    /// 故回滚后旧曲 EOS/Error 仍属当前代，不会被过滤）；未确认仅 warn。
    ///
    /// 已知限制：真实 GstDriver 在 LoadCommand 处理时同步置 current（乐观
    /// ACK），坏 URI 的真装载失败表现为**同代 Error**（仅发布、不回滚），
    /// 事务回滚路径当前仅由超时模型（FakeDriver）覆盖。
    async fn rollback_load(&mut self, prev: AppliedLoad, position: std::time::Duration) {
        let id = prev.track.id.clone();
        self.driver.load(hmp_player_gst::LoadRequest {
            track: prev.track.clone(),
            uri: prev.uri,
            quality: prev.quality,
            load_gen: prev.load_gen,
        });
        if self.wait_current_applied(&id).await.is_ok() {
            self.driver.seek(position);
            self.driver.play();
        } else {
            tracing::warn!("回滚装载未确认（旧曲可能无法恢复）");
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
mod tests {
    use super::*;
    use crate::player::{EngineError, ResolvedTrack};
    use hmp_core::{LoopMode, PlaybackState, PlaybackStatus, PlayerCommand, Track, TrackId};
    use hmp_player_gst::LoadRequest;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use tokio::sync::{broadcast, watch};

    /// 记录 load 的 uri 与装载代际（uri, load_gen）与收到的命令。
    pub struct FakeDriver {
        pub state_tx: watch::Sender<PlaybackState>,
        pub events_tx: broadcast::Sender<PlayerEvent>,
        pub loads: Mutex<Vec<(String, u64)>>,
        pub commands: Mutex<Vec<PlayerCommand>>,
        /// 置位后下一次 load 不更新 current（模拟驱动装载失败 → wait 超时）。
        pub fail_next_load: std::sync::atomic::AtomicBool,
        /// 剩余失败次数（连续多次装载失败，如回滚也失败；0=不失败）。
        pub fail_remaining: std::sync::atomic::AtomicU32,
    }

    impl FakeDriver {
        pub fn new() -> (
            Arc<Self>,
            watch::Receiver<PlaybackState>,
            broadcast::Receiver<PlayerEvent>,
        ) {
            let (state_tx, state_rx) = watch::channel(PlaybackState::default());
            let (events_tx, events_rx) = broadcast::channel(16);
            let d = Arc::new(Self {
                state_tx,
                events_tx,
                loads: Mutex::new(Vec::new()),
                commands: Mutex::new(Vec::new()),
                fail_next_load: std::sync::atomic::AtomicBool::new(false),
                fail_remaining: std::sync::atomic::AtomicU32::new(0),
            });
            (d, state_rx, events_rx)
        }
        #[allow(dead_code)] // 测试脚手架保留（行为测试目前未直接调用）
        pub fn set_status(&self, status: PlaybackStatus) {
            self.state_tx.send_modify(|s| s.status = status);
        }
        pub fn set_fail_load(&self, on: bool) {
            self.fail_next_load
                .store(on, std::sync::atomic::Ordering::SeqCst);
        }
        /// 连续 n 次装载失败（回滚重载也失败等场景）。
        pub fn set_fail_loads(&self, n: u32) {
            self.fail_remaining
                .store(n, std::sync::atomic::Ordering::SeqCst);
        }
        pub fn emit(&self, ev: PlayerEvent) {
            let _ = self.events_tx.send(ev);
        }
        /// 仅 URI 列表（断言便捷；loads 同时记录装载代际）。
        pub fn load_uris(&self) -> Vec<String> {
            self.loads
                .lock()
                .unwrap()
                .iter()
                .map(|(u, _)| u.clone())
                .collect()
        }
    }

    impl PlaybackDriver for FakeDriver {
        fn load(&self, request: LoadRequest) {
            self.loads
                .lock()
                .unwrap()
                .push((request.uri.clone(), request.load_gen));
            if self
                .fail_next_load
                .swap(false, std::sync::atomic::Ordering::SeqCst)
                || self
                    .fail_remaining
                    .fetch_update(
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                        |n| n.checked_sub(1),
                    )
                    .is_ok()
            {
                return; // 失败模拟：current 不更新 → wait_current_applied 超时
            }
            // 模拟真实驱动：装载即把 current 更新为目标曲目并进入 Playing。
            let (track, quality) = (request.track.clone(), request.quality);
            self.state_tx.send_modify(|s| {
                s.status = PlaybackStatus::Playing;
                s.current = Some(track);
                s.actual_quality = Some(quality);
            });
        }
        fn play(&self) {}
        fn pause(&self) {}
        fn seek(&self, p: std::time::Duration) {
            self.commands.lock().unwrap().push(PlayerCommand::Seek(p));
        }
        fn stop(&self) {
            self.commands.lock().unwrap().push(PlayerCommand::Stop);
        }
        fn set_volume(&self, _v: f64) {}
        fn command(&self, cmd: PlayerCommand) {
            self.commands.lock().unwrap().push(cmd);
        }
        fn shutdown(&self) {}
        fn subscribe_state(&self) -> watch::Receiver<PlaybackState> {
            self.state_tx.subscribe()
        }
        fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent> {
            self.events_tx.subscribe()
        }
    }

    /// 固定返回曲目列表的解析器（不触网）。
    #[derive(Debug)]
    pub struct FakeResolver {
        pub stubs: Mutex<Vec<Vec<hmp_core::TrackStub>>>, // 每次 resolve_source_ids 弹出一个列表
    }

    impl FakeResolver {
        /// 便捷构造：TrackId 列表（stub 元数据自动生成，title=id）。
        pub fn new(ids: Vec<Vec<TrackId>>) -> Arc<Self> {
            Arc::new(Self {
                stubs: Mutex::new(ids.into_iter().map(stub_list).collect()),
            })
        }

        /// 带元数据的构造（投影层测试用）。
        pub fn new_stubs(stubs: Vec<Vec<hmp_core::TrackStub>>) -> Arc<Self> {
            Arc::new(Self {
                stubs: Mutex::new(stubs),
            })
        }
    }

    /// TrackId 列表 → stub 列表（title 回退 id）。
    fn stub_list(ids: Vec<TrackId>) -> Vec<hmp_core::TrackStub> {
        ids.into_iter()
            .map(|id| hmp_core::TrackStub {
                id: id.clone(),
                title: id.to_string(),
                artists: Vec::new(),
                album: None,
                duration_ms: None,
            })
            .collect()
    }

    impl SourceResolver for FakeResolver {
        fn resolve_source_ids(
            &self,
            _src: &hmp_core::PlayRequest,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>>
        {
            Box::pin(async { Ok(self.stubs.lock().unwrap().remove(0)) })
        }
        fn resolve_track(
            &self,
            id: &TrackId,
        ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
            // 克隆 id：让 future 持有数据，不借用参数（返回类型生命周期为 `&self`）。
            let id = id.clone();
            Box::pin(async move {
                Ok(ResolvedTrack {
                    track: Track {
                        id: id.clone(),
                        title: format!("t-{id}"),
                        artists: vec![],
                        album: None,
                        duration: Some(std::time::Duration::from_secs(60)),
                        cover: None,
                        url: Some(format!("fake://{id}")),
                        available_qualities: vec![],
                    },
                    uri: format!("fake://{id}"),
                    media: None,
                    quality: hmp_core::AudioQuality::Mp3_128,
                })
            })
        }
    }

    /// 源解析即失败的解析器（Finding 2 测试）。
    #[derive(Debug)]
    pub struct FailResolver {
        pub err: EngineError,
    }

    /// 测试用 `EngineError` 克隆（Error 未派生 Clone）。
    fn clone_error(e: &EngineError) -> EngineError {
        match e {
            EngineError::NotLoggedIn => EngineError::NotLoggedIn,
            EngineError::TrackNotFound => EngineError::TrackNotFound,
            EngineError::PlaylistNotFound(m) => EngineError::PlaylistNotFound(m.clone()),
            EngineError::QualityUnavailable(m) => EngineError::QualityUnavailable(m.clone()),
            EngineError::Timeout => EngineError::Timeout,
            EngineError::Internal(m) => EngineError::Internal(m.clone()),
        }
    }

    impl FailResolver {
        pub fn new(err: EngineError) -> Arc<Self> {
            Arc::new(Self { err })
        }
    }

    impl SourceResolver for FailResolver {
        fn resolve_source_ids(
            &self,
            _src: &hmp_core::PlayRequest,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>>
        {
            let err = clone_error(&self.err);
            Box::pin(async move { Err(err) })
        }
        fn resolve_track(
            &self,
            _id: &TrackId,
        ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
            let err = clone_error(&self.err);
            Box::pin(async move { Err(err) })
        }
    }

    /// 源解析成功、但指定曲目 resolve_track 失败的解析器（装载失败事务测试）。
    #[derive(Debug)]
    pub struct PartialFailResolver {
        pub stubs: Mutex<Vec<Vec<hmp_core::TrackStub>>>,
        pub fail_ids: Vec<TrackId>,
        pub err: EngineError,
    }

    impl PartialFailResolver {
        pub fn new(ids: Vec<Vec<TrackId>>, fail_ids: Vec<TrackId>) -> Arc<Self> {
            Arc::new(Self {
                stubs: Mutex::new(ids.into_iter().map(stub_list).collect()),
                fail_ids,
                err: EngineError::TrackNotFound,
            })
        }
    }

    impl SourceResolver for PartialFailResolver {
        fn resolve_source_ids(
            &self,
            _src: &hmp_core::PlayRequest,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>>
        {
            Box::pin(async { Ok(self.stubs.lock().unwrap().remove(0)) })
        }
        fn resolve_track(
            &self,
            id: &TrackId,
        ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
            let id = id.clone();
            let fail = self.fail_ids.contains(&id);
            let err = clone_error(&self.err);
            Box::pin(async move {
                if fail {
                    return Err(err);
                }
                Ok(ResolvedTrack {
                    track: Track {
                        id: id.clone(),
                        title: format!("t-{id}"),
                        artists: vec![],
                        album: None,
                        duration: Some(std::time::Duration::from_secs(60)),
                        cover: None,
                        url: Some(format!("fake://{id}")),
                        available_qualities: vec![],
                    },
                    uri: format!("fake://{id}"),
                    media: None,
                    quality: hmp_core::AudioQuality::Mp3_128,
                })
            })
        }
    }

    /// 测试用 engine 启动辅助。
    async fn start_engine(
        driver: Arc<FakeDriver>,
        resolver: Arc<dyn SourceResolver>,
    ) -> (EngineHandle, watch::Receiver<hmp_core::DaemonState>) {
        let handle = PlaybackEngine::start(driver, resolver, Arc::new(|| true));
        let st = handle.state_rx.clone();
        (handle, st)
    }

    /// 带媒体库的引擎（B4 会话写库测试）。
    async fn start_engine_with_library(
        driver: Arc<FakeDriver>,
        resolver: Arc<dyn SourceResolver>,
        library: std::sync::Arc<std::sync::Mutex<hmp_storage::LibraryDb>>,
    ) -> (EngineHandle, watch::Receiver<hmp_core::DaemonState>) {
        let handle =
            PlaybackEngine::start_with_library(driver, resolver, Arc::new(|| true), Some(library));
        let st = handle.state_rx.clone();
        (handle, st)
    }

    /// 等待命令循环消化完已投递命令（yield 数次）。
    async fn wait_idle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    /// 解析器弹出一个列表；Play 后队列被替换。
    #[tokio::test]
    async fn play_replaces_queue_and_loads_first() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![
            TrackId::new("a"),
            TrackId::new("b"),
            TrackId::new("c"),
        ]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
        assert_eq!(handle.queue_rx.borrow().tracks.len(), 3);
        assert_eq!(driver.load_uris(), vec!["fake://a"]);
    }

    /// Next 命令 → 队列前进并加载下一首。
    #[tokio::test]
    async fn next_command_navigates_queue() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![
            TrackId::new("a"),
            TrackId::new("b"),
            TrackId::new("c"),
        ]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        handle
            .cmd(Request::Command(PlayerCommand::Next))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(1));
        assert_eq!(driver.load_uris(), vec!["fake://a", "fake://b"]);
    }

    /// prev 恒跳上一首（不做 >3s 回开头）。
    #[tokio::test]
    async fn prev_always_goes_previous_track() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![
            TrackId::new("a"),
            TrackId::new("b"),
            TrackId::new("c"),
        ]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        handle
            .cmd(Request::Command(PlayerCommand::Next))
            .await
            .unwrap();
        wait_idle().await;
        handle
            .cmd(Request::Command(PlayerCommand::Next))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(2));
        handle
            .cmd(Request::Command(PlayerCommand::Previous))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(1));
        assert_eq!(
            driver.load_uris(),
            vec!["fake://a", "fake://b", "fake://c", "fake://b"]
        );
    }

    /// Ended 事件 → 自动续播下一首。
    #[tokio::test]
    async fn ended_event_auto_advances() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        driver.emit(PlayerEvent::PlaybackEnded { load_gen: 1 }); // 当前代（首载 gen=1）
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(1));
        assert_eq!(driver.load_uris(), vec!["fake://a", "fake://b"]);
    }

    /// Ended 且队列到头（None 循环）→ 保持空闲，不再加载。
    #[tokio::test]
    async fn ended_with_no_next_stays_idle() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        driver.emit(PlayerEvent::PlaybackEnded { load_gen: 1 }); // 当前代（首载 gen=1）
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
        assert_eq!(driver.loads.lock().unwrap().len(), 1); // 只加载过一次
    }

    /// MPRIS OpenUri：file:// 转为本地播放请求（C4）。
    #[tokio::test]
    async fn open_uri_file_plays_via_play_source() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("local:/tmp/x.mp3")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::OpenUri("file:///tmp/x.mp3".into()))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.queue_rx.borrow().tracks.len(), 1);
        assert_eq!(
            handle.queue_rx.borrow().tracks[0].as_ref(),
            "local:/tmp/x.mp3"
        );
        assert_eq!(
            driver.load_uris().last(),
            Some(&"fake://local:/tmp/x.mp3".to_string())
        );
    }

    /// MPRIS OpenUri：非 file:// → 错误上浮（last_error）。
    #[tokio::test]
    async fn open_uri_unsupported_scheme_sets_error() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::OpenUri("https://x/1.mp3".into()))
            .await
            .unwrap();
        wait_idle().await;
        let state = handle.state_rx.borrow();
        assert!(state.last_error.is_some());
        assert!(handle.queue_rx.borrow().tracks.is_empty());
    }

    /// caps：shuffle 与循环正交——None 模式队尾开 shuffle 仍不可 Next（不再隐含列表循环）；
    /// 开 List 循环后恒可。MPRIS 能力与实际队列裁决一致。
    #[tokio::test]
    async fn caps_allow_next_at_tail_when_shuffled() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![
            TrackId::new("a"),
            TrackId::new("b"),
            TrackId::new("c"),
        ]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        // 走到队尾 c
        handle
            .cmd(Request::Command(PlayerCommand::Next))
            .await
            .unwrap();
        wait_idle().await;
        handle
            .cmd(Request::Command(PlayerCommand::Next))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(2));
        assert!(!handle.state_rx.borrow().caps.can_go_next); // None 模式队尾不可
        handle
            .cmd(Request::Command(PlayerCommand::SetShuffle(true)))
            .await
            .unwrap();
        wait_idle().await;
        // shuffle 只改顺序：None 循环队尾仍不可 next（旧行为洗牌即隐含列表循环）。
        assert!(!handle.state_rx.borrow().caps.can_go_next);
        assert!(handle.state_rx.borrow().caps.can_go_previous);
        // List 循环恒可。
        handle
            .cmd(Request::Command(PlayerCommand::SetLoopMode(LoopMode::List)))
            .await
            .unwrap();
        wait_idle().await;
        assert!(handle.state_rx.borrow().caps.can_go_next);
        assert!(handle.state_rx.borrow().caps.can_go_previous);
    }

    /// List 循环：Ended 后回绕到第一首。
    #[tokio::test]
    async fn list_loop_wraps_on_ended() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        handle
            .cmd(Request::Command(PlayerCommand::SetLoopMode(LoopMode::List)))
            .await
            .unwrap();
        wait_idle().await;
        driver.emit(PlayerEvent::PlaybackEnded { load_gen: 1 }); // a → b（首载 gen=1）
        wait_idle().await;
        driver.emit(PlayerEvent::PlaybackEnded { load_gen: 2 }); // b → a（gen=2）
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
        assert_eq!(driver.load_uris(), vec!["fake://a", "fake://b", "fake://a"]);
    }

    /// PlayNext：插入到当前曲之后，current 定位到插入位置（队列中部也正确）。
    #[tokio::test]
    async fn playnext_inserts_after_current_mid_queue() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![
            vec![TrackId::new("a"), TrackId::new("b"), TrackId::new("c")],
            vec![TrackId::new("x")],
        ]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        handle
            .cmd(Request::Command(PlayerCommand::Next))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(1)); // 当前为 b
        handle
            .cmd(Request::PlayNext(PlayRequest::Track(TrackId::new("x"))))
            .await
            .unwrap();
        wait_idle().await;
        let state = handle.state_rx.borrow();
        assert_eq!(state.queue.current, Some(2)); // 指向插入的 x
        assert_eq!(
            handle.queue_rx.borrow().tracks,
            vec![
                TrackId::new("a"),
                TrackId::new("b"),
                TrackId::new("x"),
                TrackId::new("c")
            ]
        );
        assert_eq!(driver.load_uris().last(), Some(&"fake://x".to_string()));
    }

    /// `hmp playnext playlist:<id>`：整片歌单插入当前曲之后（旧代码只插 ids[0]）。
    #[tokio::test]
    async fn playnext_inserts_full_playlist_after_current() {
        let (driver, _st, _ev) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![
            vec![TrackId::new("a")],                                       // Play(a)
            vec![TrackId::new("x"), TrackId::new("y"), TrackId::new("z")], // PlayNext(playlist)
        ]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        handle
            .cmd(Request::Command(PlayerCommand::Next)) // a → 定位到 b? 无 b：直接播完场景
            .await
            .unwrap();
        wait_idle().await;
        // 回到 a（None 模式 a 之后无曲，Next 不跳）
        let _ = handle.state_rx.borrow().queue.current;
        handle
            .cmd(Request::PlayNext(PlayRequest::Playlist(
                hmp_core::PlaylistId::new("p"),
            )))
            .await
            .unwrap();
        wait_idle().await;
        let state = handle.state_rx.borrow();
        // 整片插入：x y z 全在队列且紧跟 a 之后，当前播放 x。
        assert_eq!(
            handle.queue_rx.borrow().tracks,
            vec![
                TrackId::new("a"),
                TrackId::new("x"),
                TrackId::new("y"),
                TrackId::new("z")
            ]
        );
        assert_eq!(state.queue.current, Some(1)); // 当前 = x
        assert_eq!(driver.load_uris().last(), Some(&"fake://x".to_string()));
    }

    /// 媒体库写库（B4）：Play 开启会话 → Next 关闭(reason=next)并开启新会话 → Quit 关闭(reason=quit)。
    #[tokio::test]
    async fn play_sessions_persist_to_library() {
        use hmp_storage::LibraryDb;

        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![
            vec![TrackId::new("a"), TrackId::new("b")],
            vec![TrackId::new("c")],
        ]);
        let library = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let (handle, _st) =
            start_engine_with_library(driver.clone(), resolver, library.clone()).await;

        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        {
            let mut lib = library.lock().unwrap();
            let recent = lib.recent_plays(10).unwrap();
            assert_eq!(recent.len(), 1);
            assert_eq!(recent[0].title, "t-a");
            assert_eq!(recent[0].ended_at, None, "会话未结束");
        }

        handle
            .cmd(Request::Command(PlayerCommand::Next))
            .await
            .unwrap();
        wait_idle().await;
        {
            let mut lib = library.lock().unwrap();
            let recent = lib.recent_plays(10).unwrap();
            assert_eq!(recent.len(), 2);
            // 最新（b）未结束；a 的会话以 next 关闭。
            assert_eq!(recent[0].title, "t-b");
            assert_eq!(recent[0].ended_at, None);
            assert_eq!(recent[1].title, "t-a");
            assert_eq!(recent[1].reason, "next");
            assert!(recent[1].ended_at.is_some());
        }

        handle.cmd(Request::Quit).await.unwrap();
        wait_idle().await;
        {
            let mut lib = library.lock().unwrap();
            let recent = lib.recent_plays(10).unwrap();
            assert_eq!(recent[0].reason, "quit", "退出时关闭当前会话");
            assert!(recent[0].ended_at.is_some());
        }
    }

    /// `hmp quit`（Request::Quit）→ 引擎退出 → 终止信号置位（serve 据此优雅退出，spec §6）。
    /// Finding 7：终止信号为 sticky watch——晚到/先建的接收者都能立即看到 true。
    #[tokio::test]
    async fn quit_shuts_down_engine() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle.cmd(Request::Quit).await.unwrap();
        // 引擎退出后终止信号须在 1s 内置位（`run()` 退出路径 send(true)）。
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            let mut term = handle.terminated.clone();
            if *term.borrow() {
                return;
            }
            let _ = term.changed().await;
            assert!(*term.borrow(), "终止信号应为 true");
        })
        .await
        .expect("quit 后引擎终止信号 1s 内未置位");
        // 引擎退出后向命令通道发消息不再成功（发送端仍可发，但引擎不再消费——不断言；
        // 断言驱动已 shutdown）
        assert!(driver.commands.lock().unwrap().is_empty()); // shutdown 不产生命令
    }

    /// 装载应用有延迟的驱动（模拟真实 GStreamer 异步管道：load() 返回后
    /// 驱动任务才更新 current）。
    struct SlowDriver {
        inner: Arc<FakeDriver>,
    }

    impl SlowDriver {
        fn new() -> (
            Arc<Self>,
            watch::Receiver<PlaybackState>,
            broadcast::Receiver<PlayerEvent>,
        ) {
            let (inner, sr, er) = FakeDriver::new();
            (Arc::new(Self { inner }), sr, er)
        }
    }

    impl PlaybackDriver for SlowDriver {
        fn load(&self, request: LoadRequest) {
            // 只记录 uri，不调用 inner.load（inner 已同步应用）：
            // 异步 150ms 后才把 current 更新为装载曲目。
            self.inner
                .loads
                .lock()
                .unwrap()
                .push((request.uri.clone(), request.load_gen));
            let st = self.inner.state_tx.clone();
            let track = request.track.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                st.send_modify(|s| {
                    s.status = PlaybackStatus::Playing;
                    s.current = Some(track);
                });
            });
        }
        fn play(&self) {}
        fn pause(&self) {}
        fn seek(&self, _p: std::time::Duration) {}
        fn stop(&self) {
            self.inner.command(PlayerCommand::Stop);
        }
        fn set_volume(&self, _v: f64) {}
        fn command(&self, cmd: PlayerCommand) {
            self.inner.command(cmd);
        }
        fn shutdown(&self) {}
        fn subscribe_state(&self) -> watch::Receiver<PlaybackState> {
            self.inner.subscribe_state()
        }
        fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent> {
            self.inner.subscribe_events()
        }
    }

    /// resolve_source_ids 有延迟的解析器（模拟歌单分页网络解析）。
    #[derive(Debug)]
    struct DelayResolver {
        inner: Arc<FakeResolver>,
        delay: std::time::Duration,
    }

    impl SourceResolver for DelayResolver {
        fn resolve_source_ids(
            &self,
            src: &PlayRequest,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>>
        {
            let inner = self.inner.clone();
            let delay = self.delay;
            let src = src.clone();
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                inner.resolve_source_ids(&src).await
            })
        }
        fn resolve_track(
            &self,
            id: &TrackId,
        ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
            self.inner.resolve_track(id)
        }
    }

    /// Bug 1（seq 受理即前置自增）：解析期间 seq 已推进、状态仍 Empty，
    /// CLI 首个轮询即误报「后端空闲」。修复：seq 在命令完成后才推进。
    #[tokio::test]
    async fn seq_does_not_advance_while_source_resolving() {
        let (driver, _sr, _er) = SlowDriver::new();
        let resolver = Arc::new(DelayResolver {
            inner: FakeResolver::new(vec![vec![TrackId::new("a")]]),
            delay: std::time::Duration::from_millis(200),
        });
        let handle = PlaybackEngine::start(driver.clone(), resolver, Arc::new(|| true));
        let st = handle.state_rx.clone();
        let seq0 = st.borrow().seq;

        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        // 命令在途：seq 必须保持边界值（CLI 依赖这一点继续轮询）。
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            st.borrow().seq,
            seq0,
            "解析未完成时 seq 不得推进（Bug 1：CLI 在 Empty 窗口误报）"
        );
        // 完成后：seq 越过边界，且首个 seq>seq0 的发布不得是「无错误的 Empty」
        // （Bug 1：CLI 在 Empty 窗口误报「后端空闲」）。
        // 引擎必须先装载完再推进代际。
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut saw_advanced = false;
        loop {
            {
                let s = st.borrow();
                if s.seq > seq0 {
                    if !saw_advanced {
                        saw_advanced = true;
                        assert_eq!(
                            s.playback.status,
                            PlaybackStatus::Playing,
                            "seq 首次推进时的发布不得是 Empty（Bug 1：CLI 误报「后端空闲」）"
                        );
                        assert_eq!(
                            s.playback.current.as_ref().map(|t| t.id.clone()),
                            Some(TrackId::new("a")),
                            "seq 首次推进时当前曲目应为新曲"
                        );
                    }
                    if s.playback.status == PlaybackStatus::Playing {
                        break;
                    }
                }
            }
            assert!(tokio::time::Instant::now() < deadline, "3s 内未完成装载");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(driver.inner.loads.lock().unwrap().len(), 1);
    }

    /// 旧代 EOS（已换下曲目的迟到事件）不得触发换曲——不再依赖 500ms 窗口。
    #[tokio::test]
    async fn stale_gen_eos_is_ignored() {
        let (driver, _sr, _er) = FakeDriver::new();
        // 两个列表：Play(a) 与 Play(b) 各消耗一个（此前单列表会在第二次
        // resolve_source_ids 时 remove(0) panic 引擎线程，测试恒真）。
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")], vec![TrackId::new("b")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        // 手动换到 b（gen=2）。
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("b"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(
            handle
                .state_rx
                .borrow()
                .playback
                .current
                .as_ref()
                .unwrap()
                .id,
            TrackId::new("b"),
            "前置：当前应为 b"
        );
        let loads_before = driver.loads.lock().unwrap().len();
        // 旧代 EOS（gen=1）到达：任何时刻都应忽略（不换曲、不置 Idle）。
        driver.emit(PlayerEvent::PlaybackEnded { load_gen: 1 });
        wait_idle().await;
        let s = handle.state_rx.borrow();
        assert_eq!(
            s.phase,
            hmp_core::EnginePhase::Playing,
            "旧代 EOS 不得把阶段置 Idle（若过滤失效 on_ended 会置 Idle）"
        );
        assert_eq!(
            s.playback.current.as_ref().unwrap().id,
            TrackId::new("b"),
            "旧代 EOS 不得换曲"
        );
        assert_eq!(
            driver.loads.lock().unwrap().len(),
            loads_before,
            "旧代 EOS 不得触发任何新装载"
        );
    }

    /// 旧代 Error（已换下曲目的迟到错误事件）不得进入状态（last_error/阶段不受污染）。
    #[tokio::test]
    async fn stale_gen_error_is_ignored() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")], vec![TrackId::new("b")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("b"))))
            .await
            .unwrap();
        wait_idle().await;
        // 旧代错误（gen=1）：不得写入 last_error、不得改变阶段。
        driver.emit(PlayerEvent::Error {
            load_gen: 1,
            error: hmp_core::HmpError::Playback("stale".into()),
        });
        wait_idle().await;
        let s = handle.state_rx.borrow();
        assert!(s.last_error.is_none(), "旧代错误不得进入 last_error");
        assert_eq!(s.phase, hmp_core::EnginePhase::Playing);
        assert_eq!(
            s.playback.current.as_ref().unwrap().id,
            TrackId::new("b"),
            "旧代错误不得改变当前曲"
        );
    }

    /// 同代 EOS = 真实曲尾：装载完成后立即到达也须续播（旧 500ms 窗口会丢短曲）。
    #[tokio::test]
    async fn same_gen_eos_advances_immediately() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        let load_gen = driver.loads.lock().unwrap()[0].1; // 首载 gen=1
        driver.emit(PlayerEvent::PlaybackEnded { load_gen });
        wait_idle().await;
        let s = handle.state_rx.borrow();
        assert_eq!(
            s.playback.current.as_ref().unwrap().id,
            TrackId::new("b"),
            "同代 EOS 应立即续播"
        );
    }

    /// 换曲装载失败（驱动未应用新曲）：队列回滚、尽力重载上一曲（恢复到旧位置）。
    #[tokio::test]
    async fn failed_load_rolls_back_to_previous_track() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")], vec![TrackId::new("b")]]);
        let handle = PlaybackEngine::start_with_options(
            driver.clone(),
            resolver,
            Arc::new(|| true),
            None,
            std::time::Duration::from_millis(300),
        );
        // 先成功播放 a（gen=1）。
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(driver.loads.lock().unwrap().len(), 1);
        // 让 a 位置前进（回滚后应 seek 回此处）。
        driver
            .state_tx
            .send_modify(|s| s.position = std::time::Duration::from_secs(12));
        // 换 b 但装载失败（不更新 current → wait 超时）。
        driver.set_fail_load(true);
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("b"))))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let loads = driver.load_uris();
        // 失败装载本身记录一条（fake://b），随后回滚重载一条（fake://a）。
        assert_eq!(loads.len(), 3, "失败装载 + 回滚重载");
        assert_eq!(loads[2], "fake://a", "回滚应重载上一曲 a");
        assert!(
            driver
                .commands
                .lock()
                .unwrap()
                .contains(&PlayerCommand::Seek(std::time::Duration::from_secs(12))),
            "回滚应 seek 回旧位置"
        );
        // play_source 失败路径 restore_phase_after_failure：旧曲 a 仍在 current → Playing。
        assert_eq!(
            handle.state_rx.borrow().phase,
            hmp_core::EnginePhase::Playing
        );
        // 队列保持 a（play_source 失败路径 restore_state）。
        assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
    }

    /// 首次装载失败：无上一曲可回滚，仅发布失败后恢复 Idle（无 current）。
    #[tokio::test]
    async fn first_load_failure_has_no_rollback() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")]]);
        let handle = PlaybackEngine::start_with_options(
            driver.clone(),
            resolver,
            Arc::new(|| true),
            None,
            std::time::Duration::from_millis(300),
        );
        driver.set_fail_load(true);
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert_eq!(driver.loads.lock().unwrap().len(), 1, "首次装载失败无回滚");
        // 无 current → restore_phase_after_failure 置 Idle。
        assert_eq!(handle.state_rx.borrow().phase, hmp_core::EnginePhase::Idle);
    }

    /// 解析失败时阶段 → Failed，随后恢复为 Playing（旧曲继续播放）。
    #[tokio::test]
    async fn phase_transitions_on_load_failure() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = PartialFailResolver::new(
            vec![vec![TrackId::new("a")], vec![TrackId::new("b")]],
            vec![TrackId::new("b")],
        );
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(
            handle.state_rx.borrow().phase,
            hmp_core::EnginePhase::Playing
        );
        // 换曲装载失败：发布 Failed，随后回滚恢复 Playing（旧曲仍在播）。
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("b"))))
            .await
            .unwrap();
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert!(st.last_error.is_some());
        assert_eq!(
            st.phase,
            hmp_core::EnginePhase::Playing,
            "回滚后恢复 Playing"
        );
        assert_eq!(
            st.playback.current.as_ref().map(|t| t.id.as_ref()),
            Some("a")
        );
    }

    /// Bug 2（状态滞后）：seq 推进时复合状态必须已反映新曲（而非旧曲）。
    #[tokio::test]
    async fn seq_advance_implies_new_track_applied() {
        let (driver, _sr, _er) = SlowDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("b")]]);
        let handle = PlaybackEngine::start(driver.clone(), resolver, Arc::new(|| true));
        let st = handle.state_rx.clone();
        let seq0 = st.borrow().seq;

        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("b"))))
            .await
            .unwrap();
        // 等待 seq 越过边界，然后立即断言：当前曲目必须是新曲 b。
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            {
                let s = st.borrow();
                if s.seq > seq0 {
                    assert_eq!(
                        s.playback.current.as_ref().map(|t| t.id.clone()),
                        Some(TrackId::new("b")),
                        "seq 推进时状态必须已反映新曲（Bug 2：显示旧曲）"
                    );
                    break;
                }
            }
            assert!(tokio::time::Instant::now() < deadline, "3s 内未完成");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// 空源：完成态携带错误（CLI 可确定性报告，而非等到 15s 超时）。
    #[tokio::test]
    async fn empty_source_sets_error_and_advances_seq() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![]]);
        let (handle, st) = start_engine(driver.clone(), resolver).await;
        let seq0 = st.borrow().seq;

        handle
            .cmd(Request::Play(PlayRequest::Playlist(
                hmp_core::PlaylistId::new("p"),
            )))
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            {
                let s = st.borrow();
                if s.seq > seq0 {
                    assert!(s.last_error.is_some(), "空源应有错误详情");
                    assert!(handle.queue_rx.borrow().tracks.is_empty());
                    break;
                }
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Finding 1：Play/PlayNext/Next/Previous 前置位 seq（命令代际边界）。
    #[tokio::test]
    async fn play_and_navigation_bump_seq() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![
            vec![TrackId::new("a"), TrackId::new("b")],
            vec![TrackId::new("x")],
        ]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        assert_eq!(handle.state_rx.borrow().seq, 0);

        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().seq, 1);

        handle
            .cmd(Request::Command(PlayerCommand::Next))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().seq, 2);

        handle
            .cmd(Request::Command(PlayerCommand::Previous))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().seq, 3);

        handle
            .cmd(Request::PlayNext(PlayRequest::Track(TrackId::new("x"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().seq, 4);
    }

    /// Finding 2：源解析失败 → DaemonState.last_error 携带映射后的错误码与消息。
    #[tokio::test]
    async fn resolution_failure_publishes_last_error() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FailResolver::new(EngineError::PlaylistNotFound("歌单为空".into()));
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Playlist(
                hmp_core::PlaylistId::new("p1"),
            )))
            .await
            .unwrap();
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert_eq!(st.seq, 1);
        let info = st.last_error.as_ref().expect("解析失败应发布 last_error");
        assert_eq!(info.code, IpcErrorCode::PlaylistNotFound);
        assert!(info.message.contains("歌单为空"));
        assert_eq!(
            handle.queue_rx.borrow().tracks.len(),
            0,
            "失败后队列不应变化"
        );
    }

    /// Finding 2：成功换曲清空上次错误。
    #[tokio::test]
    async fn successful_play_clears_last_error() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FailResolver::new(EngineError::PlaylistNotFound("歌单为空".into()));
        let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
        handle
            .cmd(Request::Play(PlayRequest::Playlist(
                hmp_core::PlaylistId::new("p1"),
            )))
            .await
            .unwrap();
        wait_idle().await;
        assert!(handle.state_rx.borrow().last_error.is_some());
        // 换成功解析器后再次 Play：错误须清空（同一引擎启动即固定解析器，故新建引擎）。
        let resolver2 = FakeResolver::new(vec![vec![TrackId::new("a")]]);
        let handle2 = PlaybackEngine::start(driver.clone(), resolver2, Arc::new(|| true));
        handle2
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert!(handle2.state_rx.borrow().last_error.is_none());
        assert_eq!(handle2.queue_rx.borrow().tracks.len(), 1);
    }

    /// Finding 4：移除当前曲 → 立即播放接替曲（仲裁不失步）。
    #[tokio::test]
    async fn remove_current_plays_replacement_immediately() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![
            TrackId::new("a"),
            TrackId::new("b"),
            TrackId::new("c"),
        ]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(0)); // 播放 a
        assert_eq!(driver.load_uris(), vec!["fake://a"]);

        handle.cmd(Request::QueueRemove(0)).await.unwrap(); // 移除正在播的 a
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert_eq!(
            handle.queue_rx.borrow().tracks,
            vec![TrackId::new("b"), TrackId::new("c")]
        );
        assert_eq!(st.queue.current, Some(0)); // 接替曲 b 占据 0
        assert_eq!(
            driver.load_uris(),
            vec!["fake://a", "fake://b"],
            "移除当前曲应立即加载接替曲"
        );
    }

    /// Finding 4：移除当前曲且队列变空 → 停止播放。
    #[tokio::test]
    async fn remove_current_to_empty_stops_playback() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(driver.loads.lock().unwrap().len(), 1);
        handle.cmd(Request::QueueRemove(0)).await.unwrap();
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert!(handle.queue_rx.borrow().tracks.is_empty());
        assert_eq!(st.queue.current, None);
        assert!(
            driver
                .commands
                .lock()
                .unwrap()
                .contains(&PlayerCommand::Stop)
        );
        assert_eq!(driver.loads.lock().unwrap().len(), 1, "空队列不应再加载");
    }

    /// 移除当前曲但接替曲装载失败 → 回滚队列，旧曲继续播放（P1 事务语义）。
    #[tokio::test]
    async fn remove_current_rolls_back_on_replacement_load_failure() {
        let (driver, _sr, _er) = FakeDriver::new();
        // b 是接替曲：resolve_track(b) 失败。
        let resolver = PartialFailResolver::new(
            vec![vec![TrackId::new("a"), TrackId::new("b")]],
            vec![TrackId::new("b")],
        );
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(driver.load_uris(), vec!["fake://a"]);

        handle.cmd(Request::QueueRemove(0)).await.unwrap(); // 移除正在播的 a
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert_eq!(
            handle.queue_rx.borrow().tracks,
            vec![TrackId::new("a"), TrackId::new("b")],
            "装载失败应回滚：被删曲目回到原位"
        );
        assert_eq!(st.queue.current, Some(0));
        assert_eq!(
            driver.load_uris(),
            vec!["fake://a"],
            "接替曲装载失败不得加载"
        );
        assert!(
            st.last_error.is_some(),
            "装载失败详情应可见（CLI 不再把旧曲当成功）"
        );
    }

    /// `queue clear`（all=false）：保留当前曲，清除待播曲目；播放不受影响。
    #[tokio::test]
    async fn queue_clear_keeps_current_playing() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![
            TrackId::new("a"),
            TrackId::new("b"),
            TrackId::new("c"),
        ]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;

        handle
            .cmd(Request::QueueClear { all: false })
            .await
            .unwrap();
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert_eq!(
            handle.queue_rx.borrow().tracks,
            vec![TrackId::new("a")],
            "只留当前曲"
        );
        assert_eq!(st.queue.current, Some(0));
        assert!(
            !driver
                .commands
                .lock()
                .unwrap()
                .contains(&PlayerCommand::Stop),
            "clear 不停止播放"
        );
        assert_eq!(driver.loads.lock().unwrap().len(), 1, "不重新加载");
    }

    /// `queue clear --all`（all=true）：清空队列并停止（无「空队列仍在播」中间态）。
    #[tokio::test]
    async fn queue_clear_all_stops_playback() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;

        handle.cmd(Request::QueueClear { all: true }).await.unwrap();
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert!(handle.queue_rx.borrow().tracks.is_empty());
        assert_eq!(st.queue.current, None);
        assert!(
            driver
                .commands
                .lock()
                .unwrap()
                .contains(&PlayerCommand::Stop),
            "clear --all 应停止播放"
        );
    }

    /// 列表解析元数据随 Play 批量缓存进媒体库（投影层查询用）。
    /// upsert 语义：详情（resolve_track）无条件覆盖 title；artist/album/duration
    /// 走 COALESCE——stub 补充详情缺失字段（本测试 fake 详情无歌手/专辑 → 保留 stub）。
    #[tokio::test]
    async fn play_source_caches_stub_metadata() {
        use hmp_storage::LibraryDb;
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new_stubs(vec![vec![hmp_core::TrackStub {
            id: TrackId::new("mid-1"),
            title: "夜曲".into(),
            artists: vec!["周杰伦".into()],
            album: Some("十一月的萧邦".into()),
            duration_ms: Some(193_000),
        }]]);
        let library = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let (handle, _st) =
            start_engine_with_library(driver.clone(), resolver, library.clone()).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("mid-1"))))
            .await
            .unwrap();
        wait_idle().await;
        let metas = library
            .lock()
            .unwrap()
            .track_meta_batch("qq", &["mid-1".to_string()])
            .unwrap();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].title, "t-mid-1", "详情标题覆盖 stub");
        assert_eq!(metas[0].artist.as_deref(), Some("周杰伦"), "stub 歌手保留");
        assert_eq!(
            metas[0].album.as_deref(),
            Some("十一月的萧邦"),
            "stub 专辑保留"
        );
    }

    /// P1 #4：Play 新曲装载失败 → 旧曲继续播放、队列保持原状、发布错误；
    /// CLI 据此不再把旧曲目当成新请求成功（seq 推进 + last_error）。
    #[tokio::test]
    async fn play_load_failure_keeps_old_queue_and_track() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = PartialFailResolver::new(
            vec![vec![TrackId::new("a")], vec![TrackId::new("b")]],
            vec![TrackId::new("b")],
        );
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().seq, 1);
        assert_eq!(
            handle
                .state_rx
                .borrow()
                .playback
                .current
                .as_ref()
                .map(|t| t.id.clone()),
            Some(TrackId::new("a"))
        );

        // 播放 b：resolve_track(b) 失败 → 事务回滚。
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("b"))))
            .await
            .unwrap();
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert_eq!(st.seq, 2, "失败命令仍推进 seq（完成边界）");
        assert!(st.last_error.is_some(), "应发布装载失败详情");
        // 队列未替换、旧曲仍在播：状态一致，CLI 不会误报成功。
        assert_eq!(
            handle.queue_rx.borrow().tracks,
            vec![TrackId::new("a")],
            "装载失败不得替换队列"
        );
        assert_eq!(
            st.playback.current.as_ref().map(|t| t.id.clone()),
            Some(TrackId::new("a")),
            "装载失败时旧曲继续播放"
        );
        assert_eq!(st.playback.status, PlaybackStatus::Playing);
    }

    /// P1 #6：None 循环队尾 Next（无可跳目标）→ 不得先关会话（否则收听时长丢失）。
    #[tokio::test]
    async fn next_without_target_keeps_session_open() {
        use hmp_storage::LibraryDb;

        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")]]);
        let library = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let (handle, _st) =
            start_engine_with_library(driver.clone(), resolver, library.clone()).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;

        // 队列只有 a，None 循环：Next 无目标。会话必须保持打开。
        handle
            .cmd(Request::Command(PlayerCommand::Next))
            .await
            .unwrap();
        wait_idle().await;
        let mut lib = library.lock().unwrap();
        let recent = lib.recent_plays(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(
            recent[0].ended_at, None,
            "无可跳目标时不得关闭当前会话（P1 #6）"
        );
    }

    /// P1 #6：导航装载失败 → 回滚队列位置（原曲继续播放，状态一致）。
    #[tokio::test]
    async fn failed_next_rolls_back_queue_position() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = PartialFailResolver::new(
            vec![vec![TrackId::new("a"), TrackId::new("b")]],
            vec![TrackId::new("b")],
        );
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(0));

        handle
            .cmd(Request::Command(PlayerCommand::Next))
            .await
            .unwrap();
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert!(st.last_error.is_some());
        assert_eq!(
            st.queue.current,
            Some(0),
            "装载失败应回滚队列位置（不得停在未装载的 b 上）"
        );
        assert_eq!(
            st.playback.current.as_ref().map(|t| t.id.clone()),
            Some(TrackId::new("a")),
            "原曲继续播放"
        );
    }

    /// Task 5：同曲重播 = 两条独立会话（不再按 track 延续合并）；
    /// 旧会话闭合用换曲时刻的旧位置作 listened_ms。
    #[tokio::test]
    async fn replay_same_track_creates_two_sessions() {
        use hmp_storage::LibraryDb;

        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")], vec![TrackId::new("a")]]);
        let library = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let (handle, _st) =
            start_engine_with_library(driver.clone(), resolver, library.clone()).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        driver
            .state_tx
            .send_modify(|s| s.position = std::time::Duration::from_secs(30));
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        let mut lib = library.lock().unwrap();
        let recent = lib.recent_plays(10).unwrap();
        assert_eq!(
            recent.len(),
            2,
            "同曲重播 = 两条独立会话（不再按 track 延续合并）"
        );
        // recent_plays 按 started_at DESC：recent[0] 为第二次播放（open），
        // recent[1] 为第一次（以换曲时刻位置 30s 闭合）。
        assert!(recent[1].ended_at.is_some() && recent[1].listened_ms == 30_000);
        assert_eq!(recent[1].reason, "manual");
        assert!(recent[0].ended_at.is_none());
    }

    /// Task 5：手动换曲——旧会话闭合用换曲时刻的旧位置（而非新曲刚装载的 ~0）。
    #[tokio::test]
    async fn manual_change_closes_old_session_with_old_position() {
        use hmp_storage::LibraryDb;

        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")], vec![TrackId::new("b")]]);
        let library = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let (handle, _st) =
            start_engine_with_library(driver.clone(), resolver, library.clone()).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        // 位置前进到 90s（模拟播放中）。
        driver
            .state_tx
            .send_modify(|s| s.position = std::time::Duration::from_secs(90));
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("b"))))
            .await
            .unwrap();
        wait_idle().await;
        let mut lib = library.lock().unwrap();
        let recent = lib.recent_plays(10).unwrap();
        assert_eq!(recent.len(), 2, "两条独立会话");
        // recent_plays 按 started_at DESC：recent[0] 为新曲 b（open），
        // recent[1] 为旧曲 a（以换曲时刻位置 90s 闭合）。
        assert!(recent[1].ended_at.is_some(), "旧会话已闭合");
        assert_eq!(
            recent[1].listened_ms, 90_000,
            "旧会话 listened_ms 用换曲时刻位置"
        );
        assert_eq!(recent[1].reason, "manual");
        assert!(recent[0].ended_at.is_none(), "新会话保持 open");
    }

    /// Repeat One（LoopMode::Track）：同代 EOS 重播同曲——旧会话以 ended 闭合，
    /// 重播新建独立 open 会话（会话粒度，不按 track 延续合并）。
    #[tokio::test]
    async fn repeat_one_closes_and_reopens_session() {
        use hmp_storage::LibraryDb;

        let (driver, _sr, _er) = FakeDriver::new();
        // 两个列表：Play(a) 与 EOS 重播各消耗一个。
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")], vec![TrackId::new("a")]]);
        let library = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let (handle, _st) =
            start_engine_with_library(driver.clone(), resolver, library.clone()).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        handle
            .cmd(Request::Command(PlayerCommand::SetLoopMode(
                LoopMode::Track,
            )))
            .await
            .unwrap();
        wait_idle().await;
        // 同代 EOS（首载 gen=1）：on_ended → end_session("ended") 闭合第一条 →
        // advance_on_eos（Track 模式）重播同曲（gen=2）→ start_session 新建第二条。
        driver.emit(PlayerEvent::PlaybackEnded { load_gen: 1 });
        wait_idle().await;
        let mut lib = library.lock().unwrap();
        let recent = lib.recent_plays(10).unwrap();
        assert_eq!(recent.len(), 2, "Repeat One 每圈独立会话");
        assert_eq!(recent[1].reason, "ended", "旧会话以 ended 闭合");
        assert!(recent[1].ended_at.is_some());
        assert!(recent[0].ended_at.is_none(), "重播会话保持 open");
        assert_eq!(recent[0].title, recent[1].title, "同曲重播");
    }

    /// 回滚重载也失败：仅 warn（不 panic）；旧曲保持 current、错误详情可见。
    /// 注意：bool 无法表达"连续两次失败"，用 fail_remaining 计数（set_fail_loads）。
    #[tokio::test]
    async fn rollback_failure_only_warns() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")], vec![TrackId::new("b")]]);
        let handle = PlaybackEngine::start_with_options(
            driver.clone(),
            resolver,
            Arc::new(|| true),
            None,
            std::time::Duration::from_millis(300),
        );
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        // 连续两次失败：Play(b) 装载失败 + 回滚重载 a 也失败。
        driver.set_fail_loads(2);
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("b"))))
            .await
            .unwrap();
        // 装载 300ms 超时 + 回滚 300ms 超时：等待两者完成（+裕度）。
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        let s = handle.state_rx.borrow();
        assert!(s.last_error.is_some(), "装载失败详情应可见");
        // 回滚失败不 panic：旧曲 a 仍在 current → 恢复播放语义（Playing）。
        assert_eq!(
            s.playback.current.as_ref().unwrap().id,
            TrackId::new("a"),
            "回滚失败后旧曲保持 current"
        );
        assert_eq!(s.phase, hmp_core::EnginePhase::Playing);
    }

    /// Blocker 回归：回滚后旧曲恢复当前代——同代 EOS 仍触发续播。
    /// （current_gen 未复原时旧曲 EOS 被误判旧代忽略：播完不续播、会话不闭合。）
    #[tokio::test]
    async fn rollback_restores_gen_then_eos_advances() {
        let (driver, _sr, _er) = FakeDriver::new();
        // 第一个列表建队 [a, c]；第二个列表供 Play(b) 的 resolve_source_ids。
        let resolver = FakeResolver::new(vec![
            vec![TrackId::new("a"), TrackId::new("c")],
            vec![TrackId::new("b")],
        ]);
        let handle = PlaybackEngine::start_with_options(
            driver.clone(),
            resolver,
            Arc::new(|| true),
            None,
            std::time::Duration::from_millis(300),
        );
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        assert_eq!(driver.loads.lock().unwrap().len(), 1);
        // 换 b 装载失败 → 回滚重载 a（current_gen 复原为 1）。
        driver.set_fail_load(true);
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("b"))))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert!(driver.loads.lock().unwrap().len() >= 2, "应有回滚重载");
        // 回滚后同代 EOS（gen=1）必须被处理：续播到 c。
        driver.emit(PlayerEvent::PlaybackEnded { load_gen: 1 });
        wait_idle().await;
        let s = handle.state_rx.borrow();
        assert_eq!(
            s.playback.current.as_ref().unwrap().id,
            TrackId::new("c"),
            "回滚后同代 EOS 应触发续播（current_gen 复原语义）"
        );
    }

    /// P1 #5：track_row 按 provider 写 source（本地曲目不得写成 qq）。
    #[test]
    fn track_row_uses_provider_source() {
        let local = Track {
            id: TrackId::new("local:/home/u/music/a.flac"),
            title: "x".into(),
            artists: vec![],
            album: None,
            duration: None,
            cover: None,
            url: None,
            available_qualities: vec![],
        };
        let qq = Track {
            id: TrackId::new("003aQm4F3GJHZq"),
            title: "y".into(),
            artists: vec![],
            album: None,
            duration: None,
            cover: None,
            url: None,
            available_qualities: vec![],
        };
        let local_row = track_row(&local);
        let qq_row = track_row(&qq);
        assert_eq!(local_row.source, "local", "本地曲目 source 应为 local");
        assert_eq!(qq_row.source, "qq");
        assert_eq!(local_row.source_key, "local:/home/u/music/a.flac");
    }

    /// 万级队列：DaemonState 发布体积必须远小于 MAX_FRAME（队列内容不走状态帧）。
    #[tokio::test]
    async fn large_queue_publish_stays_small() {
        let (driver, _, _) = FakeDriver::new();
        let ids: Vec<TrackId> = (0..10_000)
            .map(|i| TrackId::new(format!("mid-{i}")))
            .collect();
        let resolver = FakeResolver::new(vec![ids.clone()]);
        let handle = PlaybackEngine::start(driver.clone(), resolver, Arc::new(|| true));
        handle
            .cmd(Request::Play(PlayRequest::Track(ids[0].clone())))
            .await
            .unwrap();
        wait_idle().await;
        let st = handle.state_rx.borrow().clone();
        assert_eq!(st.queue.len, 10_000);
        let frame = hmp_core::ipc::encode_frame(&st).unwrap();
        assert!(
            frame.len() < hmp_core::ipc::MAX_FRAME / 4,
            "万级队列状态帧应保持小体积，实际 {} 字节",
            frame.len()
        );
        // 完整队列仍可经 queue_rx 取到。
        assert_eq!(handle.queue_rx.borrow().tracks.len(), 10_000);
    }
}
