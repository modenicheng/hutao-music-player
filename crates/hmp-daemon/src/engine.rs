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
    /// 播放能力发布（MPRIS 订阅，Finding 9）。
    caps_tx: watch::Sender<PlaybackCapabilities>,
    /// 终止信号发布（sticky，Finding 7）。
    term_tx: watch::Sender<bool>,
}

impl PlaybackEngine {
    /// 启动引擎（spawn 主循环任务），返回句柄。
    pub fn start(
        driver: Arc<dyn PlaybackDriver>,
        resolver: Arc<dyn SourceResolver>,
        credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
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
            caps_tx,
            term_tx,
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
                            self.driver.shutdown();
                            break;
                        }
                        Request::Command(cmd) => self.handle_player_command(cmd).await,
                        Request::Play(src) => {
                            // 换曲操作前置位命令代际（Finding 1）：CLI 以 seq 前进作为边界。
                            self.seq += 1;
                            self.play_source(src, false).await;
                        }
                        Request::PlayNext(src) => {
                            self.seq += 1;
                            self.play_source(src, true).await;
                        }
                        Request::QueueAppend(src) => {
                            match self.resolver.resolve_source_ids(&src).await {
                                Ok(ids) => {
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
                            // 移除当前曲：立即播放接替曲（或空队列停止），避免仲裁失步（Finding 4）。
                            let was_current = self.queue.snapshot().current == Some(i);
                            if self.queue.remove(i) {
                                if was_current {
                                    self.publish();
                                    if let Some(id) = self.queue.current().cloned() {
                                        self.load_and_play(id).await;
                                    } else {
                                        self.last_error = None;
                                        self.driver.stop();
                                        self.publish();
                                    }
                                } else {
                                    self.publish();
                                }
                            }
                        }
                        Request::QueueClear => {
                            self.queue.clear();
                            self.publish();
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
                        Ok(PlayerEvent::PlaybackEnded) => self.on_ended().await,
                        Ok(PlayerEvent::Error(_)) => self.publish(), // 不自动跳歌（spec §7）
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
        };
        let _ = self.state_tx.send(state);
        let _ = self.caps_tx.send(caps);
    }

    async fn handle_player_command(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::Next => {
                self.seq += 1;
                self.navigate_next().await;
            }
            PlayerCommand::Previous => {
                self.seq += 1;
                self.navigate_prev().await;
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
            PlayerCommand::LoadAndPlay(_) => {
                // 队列场景不使用（CLI/桌面按 id 走 Play 请求）；忽略。
            }
            other => self.driver.command(other), // Play/Pause/Stop/Seek/Volume/TogglePlay 直通驱动
        }
    }

    async fn navigate_next(&mut self) {
        if let Some(id) = self.queue.skip_next() {
            self.publish();
            self.load_and_play(id).await;
        }
    }

    async fn navigate_prev(&mut self) {
        if let Some(id) = self.queue.prev_track() {
            self.publish();
            self.load_and_play(id).await;
        }
    }

    /// Play / PlayNext：解析源 → 替换/插入队列 → 加载当前。
    async fn play_source(&mut self, src: PlayRequest, playnext: bool) {
        let ids = match self.resolver.resolve_source_ids(&src).await {
            Ok(ids) => ids,
            Err(e) => {
                // 解析失败 → 发布错误详情（Finding 2）。
                self.last_error = Some(error_info(&e));
                self.publish();
                return;
            }
        };
        if ids.is_empty() {
            self.last_error = None;
            self.publish();
            return;
        }
        if playnext {
            // 整片插入当前曲之后（多曲目；空队列按 replace 建队）。
            if let Some(at) = self.queue.insert_after_current(ids.clone()) {
                self.queue.set_current(at); // 当前曲定位到插入的首曲（开始播放它）
            }
            self.publish();
            self.load_and_play(ids[0].clone()).await;
        } else {
            self.queue.replace(ids.clone(), 0);
            self.publish();
            self.load_and_play(ids[0].clone()).await;
        }
    }

    async fn on_ended(&mut self) {
        if let Some(id) = self.queue.advance_on_eos() {
            self.publish();
            self.load_and_play(id).await;
        } else {
            self.publish();
        }
    }

    /// 解析 + 解密 + 加载 + 播放。
    async fn load_and_play(&mut self, id: TrackId) {
        // 成功路径：清除旧错误（Finding 2）。
        self.last_error = None;
        match self.resolver.resolve_track(&id).await {
            Ok(res) => {
                self.active_media = res.media; // 旧 guard 自动 Drop → 旧代理停止
                let uri = res.uri.clone();
                let quality = res
                    .track
                    .qualities
                    .first()
                    .cloned()
                    .unwrap_or(hmp_core::AudioQuality::Mp3_128);
                self.driver.load(hmp_player_gst::LoadRequest {
                    track: res.track,
                    uri,
                    quality,
                });
                self.driver.play();
                self.publish();
            }
            Err(e) => {
                tracing::error!(%e, "解析失败: {id}");
                // 队列位置保持；错误详情进入复合状态（Finding 2）。
                self.last_error = Some(error_info(&e));
                self.publish();
            }
        }
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
            self.loads.lock().unwrap().push(request.uri);
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
    pub struct FakeResolver {
        pub ids: Mutex<Vec<Vec<TrackId>>>, // 每次 resolve_source_ids 弹出一个列表
    }

    impl FakeResolver {
        pub fn new(ids: Vec<Vec<TrackId>>) -> Arc<Self> {
            Arc::new(Self {
                ids: Mutex::new(ids),
            })
        }
    }

    impl SourceResolver for FakeResolver {
        fn resolve_source_ids(
            &self,
            _src: &hmp_core::PlayRequest,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<TrackId>, EngineError>> + Send + '_>> {
            Box::pin(async { Ok(self.ids.lock().unwrap().remove(0)) })
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
                        qualities: vec![],
                    },
                    uri: format!("fake://{id}"),
                    media: None,
                })
            })
        }
    }

    /// 源解析即失败的解析器（Finding 2 测试）。
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
        ) -> Pin<Box<dyn Future<Output = Result<Vec<TrackId>, EngineError>> + Send + '_>> {
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

    /// 测试用 engine 启动辅助。
    async fn start_engine(
        driver: Arc<FakeDriver>,
        resolver: Arc<dyn SourceResolver>,
    ) -> (EngineHandle, watch::Receiver<hmp_core::DaemonState>) {
        let handle = PlaybackEngine::start(driver, resolver, Arc::new(|| true));
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
        driver.emit(PlayerEvent::PlaybackEnded);
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
        assert_eq!(driver.loads.lock().unwrap().len(), 1); // 只加载过一次
    }

    /// caps：shuffle 开启时队尾仍可 Next（引擎可回绕），MPRIS 不再误报 false。
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
        assert!(handle.state_rx.borrow().caps.can_go_next); // 洗牌可回绕
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
        driver.emit(PlayerEvent::PlaybackEnded); // a → b
        wait_idle().await;
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
}
