//! 播放引擎：命令循环 + 队列裁决 + 自动续播 + 复合状态发布（spec §4.2 `daemon.rs`）。
//!
//! 单一命令通道：所有输入适配器（socket 服务器 / tray / MPRIS）把
//! [`Request`] 发进 [`EngineHandle::command_tx`]，由引擎串行处理；
//! 单一状态出口：`watch<DaemonState>`。Next/Previous 由引擎拦截做队列
//! 导航（PlayerCore 忽略这两个命令，见 hmp-player-gst core.rs）。

use std::sync::Arc;

use hmp_core::{
    DaemonState, ErrorInfo, IpcErrorCode, PlayRequest, PlaybackCapabilities, PlaybackState,
    PlayerCommand, Request, TrackId,
};
use hmp_player_gst::PlayerEvent;
use tokio::sync::{mpsc, watch};

use crate::player::{EngineError, PlaybackDriver, SourceResolver};

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
    /// 最近一次装载完成时刻（滞后 EOS/Error 窗口判定，spec §7）。
    loaded_at: Option<std::time::Instant>,
    /// 播放能力发布（MPRIS 订阅，Finding 9）。
    caps_tx: watch::Sender<PlaybackCapabilities>,
    /// 终止信号发布（sticky，Finding 7）。
    term_tx: watch::Sender<bool>,
    /// 媒体库（播放会话写库；不可用时为 None，播放不阻断）。
    library: Option<std::sync::Arc<std::sync::Mutex<hmp_storage::LibraryDb>>>,
    /// 当前播放会话的 DB track id（供会话结束回写）。
    current_db_track: Option<i64>,
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
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (state_tx, state_rx) = watch::channel(DaemonState::default());
        let playback_rx = driver.subscribe_state();
        let (caps_tx, caps_rx) = watch::channel(PlaybackCapabilities::default());
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
            loaded_at: None,
            caps_tx,
            term_tx,
            library,
            current_db_track: None,
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
                                    let old_db_track = self.current_db_track;
                                    if let Some(id) = self.queue.current().cloned() {
                                        if self.load_and_play(id).await.is_ok() {
                                            // 装载成功：关闭命令前的旧会话（同曲延续则跳过）。
                                            if let Some(old) = old_db_track {
                                                if self.current_db_track != Some(old) {
                                                    self.close_session(old, "manual");
                                                }
                                            }
                                        } else {
                                            // 装载失败：回滚队列（被删曲目回到原位，
                                            // 旧曲继续播放）；last_error 已由 load_and_play 发布。
                                            self.queue.restore_state(saved);
                                            self.restore_phase_after_failure();
                                            self.publish(); // 回滚后重新发布（load_and_play 已发布中间态）
                                        }
                                    } else {
                                        // 空队列：确定性停止。
                                        self.end_session("manual");
                                        self.last_error = None;
                                        self.driver.stop();
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
                        Ok(PlayerEvent::PlaybackEnded) => {
                            // 滞后事件防护（spec §7）：新曲装载中（Loading）或装载完成
                            // 500ms 内到达的 EOS 属旧曲目 → 忽略，不触发换曲。
                            if self.phase == hmp_core::EnginePhase::Loading
                                || self
                                    .loaded_at
                                    .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500))
                            {
                                tracing::debug!("忽略滞后 EOS（装载窗口内）");
                            } else {
                                self.on_ended().await;
                            }
                        }
                        Ok(PlayerEvent::Error(_)) => {
                            // 不自动跳歌（spec §7）；装载窗口内的错误事件属旧曲 → 忽略
                            // （装载结果由 load_and_play 决定）。
                            if self.phase == hmp_core::EnginePhase::Loading {
                                tracing::debug!("忽略滞后错误事件（装载窗口内）");
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
    fn publish(&self) {
        let queue = self.queue.snapshot();
        let caps = PlaybackCapabilities {
            can_go_next: self.queue.can_go_next(),
            can_go_previous: self.queue.can_go_previous(),
        };
        let state = DaemonState {
            playback: self.state_rx.borrow().clone(),
            queue,
            caps,
            seq: self.seq,
            last_error: self.last_error.clone(),
            phase: self.phase,
        };
        let _ = self.state_tx.send(state);
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
        let old_db_track = self.current_db_track;
        if self.load_and_play(id).await.is_ok() {
            // 装载成功才切换会话：关闭命令前打开的会话（同曲连续播放则延续）。
            if let Some(old) = old_db_track {
                if self.current_db_track != Some(old) {
                    self.close_session(old, "next");
                }
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
        let old_db_track = self.current_db_track;
        if self.load_and_play(id).await.is_ok() {
            if let Some(old) = old_db_track {
                if self.current_db_track != Some(old) {
                    self.close_session(old, "previous");
                }
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
        let old_db_track = self.current_db_track;
        match self.load_and_play(ids[0].clone()).await {
            Ok(()) => {
                // 提交：关闭命令前打开的旧会话（同曲连续播放则延续）。
                if let Some(old) = old_db_track {
                    if self.current_db_track != Some(old) {
                        self.close_session(old, "manual");
                    }
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
    /// 同曲目连续播放（重播/循环）不新建会话——会话延续，避免同一曲目
    /// 留下两条未闭合记录（P1 会话一致性）。
    fn start_session(&mut self, track: &hmp_core::Track) {
        let Some(library) = &self.library else {
            return;
        };
        let mut library = library.lock().unwrap();
        let row = track_row(track);
        match library.upsert_track(&row) {
            Ok(track_id) => {
                if self.current_db_track == Some(track_id) {
                    return; // 同曲延续
                }
                if library.record_play_start(track_id, now_unix()).is_ok() {
                    self.current_db_track = Some(track_id);
                }
            }
            Err(e) => tracing::warn!(%e, "媒体库 upsert 失败"),
        }
    }

    /// 媒体库：结束当前播放会话（UPDATE play_events + 播放次数）。
    /// 收听时长 = 当前播放位置（位置无时长上限时原样记录）。
    fn end_session(&mut self, reason: &'static str) {
        if let Some(track_id) = self.current_db_track.take() {
            self.close_session(track_id, reason);
        }
    }

    /// 按曲目 id 关闭播放会话（事务提交路径用：先装载成功、后关闭旧会话）。
    fn close_session(&self, track_id: i64, reason: &'static str) {
        let Some(library) = &self.library else {
            return;
        };
        let listened_ms = self.state_rx.borrow().position.as_millis() as i64;
        let mut library = library.lock().unwrap();
        let end = hmp_storage::PlayEnd {
            track_id,
            ended_at: now_unix(),
            listened_ms,
            reason,
        };
        if let Err(e) = library.record_play_end(&end) {
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
                self.active_media = res.media; // 旧 guard 自动 Drop → 旧代理停止
                let uri = res.uri.clone();
                let quality = res.quality;
                let expected = res.track.id.clone();
                self.driver.load(hmp_player_gst::LoadRequest {
                    track: res.track.clone(),
                    uri,
                    quality,
                });
                self.driver.play();
                // 等待驱动应用装载（真实驱动为异步管道）：完成前发布的复合状态
                // 不得携带旧曲目（Bug 2：play-next 后显示旧曲）。
                self.wait_current_applied(&expected).await;
                // 装载完成：进入播放阶段，记录完成时刻（滞后事件窗口）。
                self.phase = hmp_core::EnginePhase::Playing;
                self.loaded_at = Some(std::time::Instant::now());
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
    /// 异步管道（真实 GStreamer）等待其装载臂发布）。5s 超时仅防御性告警。
    async fn wait_current_applied(&mut self, expected: &TrackId) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            {
                let cur = self.state_rx.borrow();
                if cur.current.as_ref().map(|t| &t.id) == Some(expected) {
                    return;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!("驱动未在 5s 内应用装载（current 未更新），继续播放流程");
                return;
            }
            if self.state_rx.changed().await.is_err() {
                return;
            }
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

    /// 记录 load 的 uri 与收到的命令。
    pub struct FakeDriver {
        pub state_tx: watch::Sender<PlaybackState>,
        pub events_tx: broadcast::Sender<PlayerEvent>,
        pub loads: Mutex<Vec<String>>,
        pub commands: Mutex<Vec<PlayerCommand>>,
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
            });
            (d, state_rx, events_rx)
        }
        #[allow(dead_code)] // 测试脚手架保留（行为测试目前未直接调用）
        pub fn set_status(&self, status: PlaybackStatus) {
            self.state_tx.send_modify(|s| s.status = status);
        }
        pub fn emit(&self, ev: PlayerEvent) {
            let _ = self.events_tx.send(ev);
        }
    }

    impl PlaybackDriver for FakeDriver {
        fn load(&self, request: LoadRequest) {
            self.loads.lock().unwrap().push(request.uri.clone());
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
        fn seek(&self, _p: std::time::Duration) {}
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
        assert_eq!(handle.state_rx.borrow().queue.tracks.len(), 3);
        assert_eq!(driver.loads.lock().unwrap().clone(), vec!["fake://a"]);
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
        assert_eq!(
            driver.loads.lock().unwrap().clone(),
            vec!["fake://a", "fake://b"]
        );
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
            driver.loads.lock().unwrap().clone(),
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
        tokio::time::sleep(std::time::Duration::from_millis(600)).await; // 滞后窗口外
        tokio::time::sleep(std::time::Duration::from_millis(600)).await; // 滞后窗口外
        driver.emit(PlayerEvent::PlaybackEnded);
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(1));
        assert_eq!(
            driver.loads.lock().unwrap().clone(),
            vec!["fake://a", "fake://b"]
        );
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
        tokio::time::sleep(std::time::Duration::from_millis(600)).await; // 滞后窗口外
        driver.emit(PlayerEvent::PlaybackEnded);
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
        let state = handle.state_rx.borrow();
        assert_eq!(state.queue.tracks.len(), 1);
        assert_eq!(state.queue.tracks[0].as_ref(), "local:/tmp/x.mp3");
        assert_eq!(
            driver.loads.lock().unwrap().last(),
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
        assert!(state.queue.tracks.is_empty());
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
        tokio::time::sleep(std::time::Duration::from_millis(600)).await; // 滞后窗口外
        driver.emit(PlayerEvent::PlaybackEnded); // a → b
        wait_idle().await;
        tokio::time::sleep(std::time::Duration::from_millis(600)).await; // 滞后窗口外
        driver.emit(PlayerEvent::PlaybackEnded); // b → a（回绕）
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
        assert_eq!(
            driver.loads.lock().unwrap().clone(),
            vec!["fake://a", "fake://b", "fake://a"]
        );
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
            state.queue.tracks,
            vec![
                TrackId::new("a"),
                TrackId::new("b"),
                TrackId::new("x"),
                TrackId::new("c")
            ]
        );
        assert_eq!(
            driver.loads.lock().unwrap().last(),
            Some(&"fake://x".to_string())
        );
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
            state.queue.tracks,
            vec![
                TrackId::new("a"),
                TrackId::new("x"),
                TrackId::new("y"),
                TrackId::new("z")
            ]
        );
        assert_eq!(state.queue.current, Some(1)); // 当前 = x
        assert_eq!(
            driver.loads.lock().unwrap().last(),
            Some(&"fake://x".to_string())
        );
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
            self.inner.loads.lock().unwrap().push(request.uri.clone());
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

    /// 装载窗口内到达的 EOS 属旧曲（滞后事件）→ 忽略，不触发换曲（spec §7）。
    #[tokio::test]
    async fn loading_window_ignores_stale_eos() {
        let (driver, _sr, _er) = SlowDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
        let handle = PlaybackEngine::start(driver.clone(), resolver, Arc::new(|| true));
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        // 装载进行中（SlowDriver 150ms 异步应用）时发出 EOS（属旧队列/旧曲的迟到事件）。
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            handle.state_rx.borrow().phase,
            hmp_core::EnginePhase::Loading,
            "装载窗口内 phase 应为 Loading"
        );
        driver.inner.emit(PlayerEvent::PlaybackEnded);
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let st = handle.state_rx.borrow();
        assert_eq!(st.queue.current, Some(0), "滞后 EOS 不得触发换曲");
        assert_eq!(
            driver.inner.loads.lock().unwrap().clone(),
            vec!["fake://a"],
            "不得加载下一首"
        );
        assert_eq!(
            st.phase,
            hmp_core::EnginePhase::Playing,
            "装载完成进入 Playing"
        );
    }

    /// 装载完成 500ms 窗口内的迟到 EOS 同样忽略；窗口外正常续播。
    #[tokio::test]
    async fn stale_eos_after_load_window_is_ignored() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        // 装载完成后的迟到 EOS（<500ms）：忽略。
        driver.emit(PlayerEvent::PlaybackEnded);
        wait_idle().await;
        assert_eq!(
            handle.state_rx.borrow().queue.current,
            Some(0),
            "迟到 EOS 忽略"
        );
        // 窗口外（>500ms）的 EOS：正常续播。
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        driver.emit(PlayerEvent::PlaybackEnded);
        wait_idle().await;
        assert_eq!(
            handle.state_rx.borrow().queue.current,
            Some(1),
            "窗口外 EOS 正常换曲"
        );
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
                    assert!(s.queue.tracks.is_empty());
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
        assert_eq!(st.queue.tracks.len(), 0, "失败后队列不应变化");
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
        assert_eq!(handle2.state_rx.borrow().queue.tracks.len(), 1);
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
        assert_eq!(driver.loads.lock().unwrap().clone(), vec!["fake://a"]);

        handle.cmd(Request::QueueRemove(0)).await.unwrap(); // 移除正在播的 a
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert_eq!(st.queue.tracks, vec![TrackId::new("b"), TrackId::new("c")]);
        assert_eq!(st.queue.current, Some(0)); // 接替曲 b 占据 0
        assert_eq!(
            driver.loads.lock().unwrap().clone(),
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
        assert!(st.queue.tracks.is_empty());
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
        assert_eq!(driver.loads.lock().unwrap().clone(), vec!["fake://a"]);

        handle.cmd(Request::QueueRemove(0)).await.unwrap(); // 移除正在播的 a
        wait_idle().await;
        let st = handle.state_rx.borrow();
        assert_eq!(
            st.queue.tracks,
            vec![TrackId::new("a"), TrackId::new("b")],
            "装载失败应回滚：被删曲目回到原位"
        );
        assert_eq!(st.queue.current, Some(0));
        assert_eq!(
            driver.loads.lock().unwrap().clone(),
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
        assert_eq!(st.queue.tracks, vec![TrackId::new("a")], "只留当前曲");
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
        assert!(st.queue.tracks.is_empty());
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
            st.queue.tracks,
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

    /// 同曲重播（Play 同一曲目）：会话延续，不新建未闭合记录。
    #[tokio::test]
    async fn same_track_replay_continues_session() {
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
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        let mut lib = library.lock().unwrap();
        let recent = lib.recent_plays(10).unwrap();
        assert_eq!(recent.len(), 1, "同曲重播不得新建会话");
        assert_eq!(recent[0].ended_at, None, "会话保持打开");
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
}
