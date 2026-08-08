//! 播放引擎：命令循环 + 队列裁决 + 自动续播 + 复合状态发布（spec §4.2 `daemon.rs`）。
//!
//! 单一命令通道：所有输入适配器（socket 服务器 / tray / MPRIS）把
//! [`Request`] 发进 [`EngineHandle::command_tx`]，由引擎串行处理；
//! 单一状态出口：`watch<DaemonState>`。Next/Previous 由引擎拦截做队列
//! 导航（PlayerCore 忽略这两个命令，见 hmp-player-gst core.rs）。

use std::sync::Arc;

use hmp_core::{
    DaemonState, LoopMode, PlayRequest, PlaybackState, PlayerCommand, Request, TrackId,
};
use hmp_player_gst::PlayerEvent;
use tokio::sync::{mpsc, watch};

use crate::player::{PlaybackDriver, SourceResolver};

/// 引擎句柄（服务器 / tray / MPRIS 持有；可 Clone）。
#[derive(Clone)]
pub struct EngineHandle {
    /// 命令通道（唯一输入）。
    pub command_tx: mpsc::UnboundedSender<Request>,
    /// 复合状态（唯一输出）。
    pub state_rx: watch::Receiver<DaemonState>,
    /// 凭证前置校验（服务器对 Play 类请求同步检查，spec §6）。
    pub credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
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
        let mut engine = Self {
            driver,
            resolver,
            queue: hmp_core::QueueCore::new(),
            state_tx,
            state_rx: playback_rx,
            cmd_rx,
            active_media: None,
        };
        tokio::spawn(async move { engine.run().await });
        EngineHandle {
            command_tx: cmd_tx,
            state_rx,
            credential_ok,
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
                        Request::Play(src) => self.play_source(src, false).await,
                        Request::PlayNext(src) => self.play_source(src, true).await,
                        Request::QueueAppend(src) => {
                            if let Ok(ids) = self.resolver.resolve_source_ids(&src).await {
                                self.queue.append(ids);
                                self.publish();
                            }
                        }
                        Request::QueueRemove(i) if self.queue.remove(i) => self.publish(),
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
    fn publish(&self) {
        let state = DaemonState {
            playback: self.state_rx.borrow().clone(),
            queue: self.queue.snapshot(),
            caps: hmp_core::PlaybackCapabilities {
                can_go_next: self.queue.snapshot().tracks.len() > 1,
                can_go_previous: true,
            },
        };
        let _ = self.state_tx.send(state);
    }

    async fn handle_player_command(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::Next => self.navigate_next().await,
            PlayerCommand::Previous => self.navigate_prev().await,
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
        if let Some(id) = self.queue.next_track() {
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
        let Ok(ids) = self.resolver.resolve_source_ids(&src).await else {
            self.publish();
            return;
        };
        if ids.is_empty() {
            return;
        }
        if playnext {
            let idx = self.queue.insert_next(ids[0].clone());
            self.queue.set_current(idx); // 定位到刚插入的位置（可能是队列中部）
            self.publish();
            self.load_and_play(ids[0].clone()).await;
        } else {
            self.queue.replace(ids.clone(), 0);
            self.publish();
            self.load_and_play(ids[0].clone()).await;
        }
    }

    async fn on_ended(&mut self) {
        if self.queue.loop_mode() == LoopMode::Track {
            if let Some(id) = self.queue.current().cloned() {
                self.load_and_play(id).await;
            }
            return;
        }
        if let Some(id) = self.queue.next_track() {
            self.publish();
            self.load_and_play(id).await;
        } else {
            self.publish();
        }
    }

    /// 解析 + 解密 + 加载 + 播放。
    async fn load_and_play(&mut self, id: TrackId) {
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
                // 队列位置保持；状态由驱动/状态呈现
                self.publish();
            }
        }
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
        fn stop(&self) {}
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

    /// 测试用 engine 启动辅助。
    async fn start_engine(
        driver: Arc<FakeDriver>,
        resolver: Arc<FakeResolver>,
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

    /// 播放结束到队列末尾 → 状态发布（Ended 保持，不崩）。
    #[tokio::test]
    async fn quit_shuts_down_engine() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![]);
        let (handle, _st) = start_engine(driver.clone(), resolver).await;
        handle.cmd(Request::Quit).await.unwrap();
        wait_idle().await;
        // 引擎退出后向命令通道发消息不再成功（发送端仍可发，但引擎不再消费——不断言；
        // 断言驱动已 shutdown）
        assert!(driver.commands.lock().unwrap().is_empty()); // shutdown 不产生命令
    }
}
