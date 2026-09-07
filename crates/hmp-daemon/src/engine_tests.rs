//! `engine.rs` 的单元测试（模块经 `#[path]` 挂在 `engine::tests`，语义不变）。

use super::*;
use crate::player::{EngineError, ResolvedTrack};
use hmp_core::{LoopMode, PlaybackState, PlaybackStatus, PlayerCommand, Track, TrackId};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use tokio::sync::{broadcast, watch};

/// 串行化改环境变量的测试（ReplayGain 配置隔离；其余测试不持有锁——
/// 它们的曲目无 RG，配置误读不影响断言）。
static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    /// `play()` 调用计数（点当前曲「确保播放」断言用）。
    pub plays: std::sync::atomic::AtomicU32,
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
            plays: std::sync::atomic::AtomicU32::new(0),
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
    fn play(&self) {
        self.plays.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    fn pause(&self) {}
    fn seek(&self, p: std::time::Duration) {
        self.commands.lock().unwrap().push(PlayerCommand::Seek(p));
    }
    fn stop(&self) {
        self.commands.lock().unwrap().push(PlayerCommand::Stop);
    }
    fn set_volume(&self, v: f64) {
        // 模拟真实驱动（core.rs 命令循环 clamp [0,1]）：补偿后音量超上限
        // 被截断（G2 Review WF2：Fake 与真实行为对齐）。
        self.state_tx.send_modify(|s| s.volume = v.clamp(0.0, 1.0));
    }
    fn command(&self, cmd: PlayerCommand) {
        self.commands.lock().unwrap().push(cmd.clone());
        // SetVolume 直通 driver：同步反映到状态（真实驱动亦然，含 clamp）。
        if let PlayerCommand::SetVolume(v) = cmd {
            self.state_tx.send_modify(|s| s.volume = v.clamp(0.0, 1.0));
        }
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
    /// resolve_track 调用计数（G2 preload 测试断言）。
    pub resolve_calls: Mutex<usize>,
    /// resolve_track 对这些 id 返回 TrackNotFound（预解析失败静默回退测试）。
    pub fail_ids: Mutex<Vec<TrackId>>,
    /// 指定 id 的 replaygain_db（G2 ReplayGain 测试；缺省 = 无 RG）。
    pub replaygain: Mutex<Vec<(TrackId, f64)>>,
}

impl FakeResolver {
    /// 便捷构造：TrackId 列表（stub 元数据自动生成，title=id）。
    pub fn new(ids: Vec<Vec<TrackId>>) -> Arc<Self> {
        Arc::new(Self {
            stubs: Mutex::new(ids.into_iter().map(stub_list).collect()),
            resolve_calls: Mutex::new(0),
            fail_ids: Mutex::new(Vec::new()),
            replaygain: Mutex::new(Vec::new()),
        })
    }

    /// resolve_track 被调用次数（含后台预解析调用）。
    pub fn resolve_calls(&self) -> usize {
        *self.resolve_calls.lock().unwrap()
    }

    /// 带元数据的构造（投影层测试用）。
    pub fn new_stubs(stubs: Vec<Vec<hmp_core::TrackStub>>) -> Arc<Self> {
        Arc::new(Self {
            stubs: Mutex::new(stubs),
            resolve_calls: Mutex::new(0),
            fail_ids: Mutex::new(Vec::new()),
            replaygain: Mutex::new(Vec::new()),
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
        let fail = self.fail_ids.lock().unwrap().contains(&id);
        let rg = self
            .replaygain
            .lock()
            .unwrap()
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, db)| *db);
        Box::pin(async move {
            *self.resolve_calls.lock().unwrap() += 1;
            if fail {
                return Err(EngineError::TrackNotFound);
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
                replaygain_db: rg,
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
                replaygain_db: None,
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
    let handle = PlaybackEngine::start_with_library(
        driver,
        resolver,
        Arc::new(|| true),
        Some(library),
        None,
    );
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

/// G2：装载后后台预解析下一首；Next 消费缓存（不二次 resolve）。
#[tokio::test]
async fn preloads_next_track_and_consumes_cache() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    // 等后台预解析完成。
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(resolver.resolve_calls(), 2, "播放 a 后应预解析 b");
    handle
        .cmd(Request::Command(PlayerCommand::Next))
        .await
        .unwrap();
    wait_idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        resolver.resolve_calls(),
        2,
        "Next 到 b 应消费预解析缓存，不再次 resolve"
    );
    assert_eq!(
        handle
            .state_rx
            .borrow()
            .playback
            .current
            .as_ref()
            .map(|t| t.id.as_ref()),
        Some("b")
    );
}

/// G2：队列整体替换（Play 新源）后，旧预解析不适用于新队列曲目；新预解析正常消费。
#[tokio::test]
async fn preload_cache_invalidated_by_queue_change() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![
        vec![TrackId::new("a"), TrackId::new("b")],
        vec![TrackId::new("c"), TrackId::new("d")],
    ]);
    let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(resolver.resolve_calls(), 2, "a + 预解析 b");
    // 队列整体替换 → 旧预解析 b 不适用于新队列（id 不符 → 不消费）。
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("c"))))
        .await
        .unwrap();
    wait_idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(resolver.resolve_calls(), 4, "c 装载 + 预解析 d");
    handle
        .cmd(Request::Command(PlayerCommand::Next))
        .await
        .unwrap();
    wait_idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        resolver.resolve_calls(),
        4,
        "d 已预解析，Next 不新增 resolve"
    );
    assert_eq!(
        handle
            .state_rx
            .borrow()
            .playback
            .current
            .as_ref()
            .map(|t| t.id.as_ref()),
        Some("d")
    );
}

/// 打磨：Repeat One（Track 模式）EOS 重播当前曲——预解析当前曲是冗余
/// 的（重复 resolve + 重复解密代理），应跳过。
#[tokio::test]
async fn preload_skipped_in_repeat_one() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
    handle
        .cmd(Request::Command(PlayerCommand::SetLoopMode(
            LoopMode::Track,
        )))
        .await
        .unwrap();
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    // 装载 a（1 次 resolve）；Track 模式不预解析（保持 1）。
    assert_eq!(resolver.resolve_calls(), 1, "Repeat One 不应预解析当前曲");
}

/// G2：预解析失败静默（不影响播放）；Next 时走正常解析（失败语义不变）。
#[tokio::test]
async fn preload_failure_is_silent_and_falls_back() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
    resolver.fail_ids.lock().unwrap().push(TrackId::new("b"));
    let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    // 预解析 b 失败（静默）：播放正常、无 last_error。
    assert_eq!(resolver.resolve_calls(), 2, "a + 预解析 b（失败也计数）");
    assert!(handle.state_rx.borrow().last_error.is_none());
    assert_eq!(
        handle
            .state_rx
            .borrow()
            .playback
            .current
            .as_ref()
            .map(|t| t.id.as_ref()),
        Some("a")
    );
    // Next → 缓存未命中（预解析失败）→ 正常 resolve b → 失败 → 回滚（a 继续播）。
    handle
        .cmd(Request::Command(PlayerCommand::Next))
        .await
        .unwrap();
    wait_idle().await;
    assert_eq!(resolver.resolve_calls(), 3, "Next 应重新 resolve b");
    assert!(
        handle.state_rx.borrow().last_error.is_some(),
        "装载失败应发布 last_error"
    );
    assert_eq!(
        handle
            .state_rx
            .borrow()
            .playback
            .current
            .as_ref()
            .map(|t| t.id.as_ref()),
        Some("a"),
        "装载失败回滚：a 继续播放"
    );
}

/// G2：EOS 续播同样消费预解析缓存（零额外 resolve）。
#[tokio::test]
async fn preload_consumed_on_eos() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(resolver.resolve_calls(), 2, "a + 预解析 b");
    // EOS（当前代 gen=1）→ 续播 b，消费预解析缓存。
    driver.emit(PlayerEvent::PlaybackEnded { load_gen: 1 });
    wait_idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        resolver.resolve_calls(),
        2,
        "EOS 续播应消费预解析缓存，不再次 resolve"
    );
    assert_eq!(
        handle
            .state_rx
            .borrow()
            .playback
            .current
            .as_ref()
            .map(|t| t.id.as_ref()),
        Some("b")
    );
}

/// G2：RG 增益在装载时叠加到用户音量；SetVolume 后仍叠加；换曲自动切换。
#[tokio::test]
// 锁仅用于串行化改 env 的测试；引擎任务从不获取该锁，无死锁风险。
#[allow(clippy::await_holding_lock)]
async fn replaygain_applied_on_load() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
    resolver
        .replaygain
        .lock()
        .unwrap()
        .push((TrackId::new("a"), 6.0)); // +6dB → factor ≈ 1.995
    let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    let expected = (10f64).powf(6.0 / 20.0);
    // 初始用户音量 1.0：补偿后 1.0×1.995 超上限 → 真实驱动 clamp 到 1.0
    //（FakeDriver 已对齐；增益本身由下方低音量断言验证）。
    let vol = handle.state_rx.borrow().playback.volume;
    assert!((vol - 1.0).abs() < 1e-9, "超上限应 clamp: {vol}");
    // 用户调音量：驱动 = 用户 × 当前曲增益（补偿不丢，且此时不触发 clamp）。
    handle
        .cmd(Request::Command(PlayerCommand::SetVolume(0.4)))
        .await
        .unwrap();
    wait_idle().await;
    let vol2 = handle.state_rx.borrow().playback.volume;
    assert!(
        (vol2 - 0.4 * expected).abs() < 1e-9,
        "SetVolume 应叠加补偿: {vol2} vs {}",
        0.4 * expected
    );
    // Next → b（无 RG）→ 增益回 1.0。
    handle
        .cmd(Request::Command(PlayerCommand::Next))
        .await
        .unwrap();
    wait_idle().await;
    let vol3 = handle.state_rx.borrow().playback.volume;
    assert!(
        (vol3 - 0.4).abs() < 1e-9,
        "无 RG 曲目应回到用户音量: {vol3}"
    );
    assert_eq!(
        handle
            .state_rx
            .borrow()
            .playback
            .current
            .as_ref()
            .map(|t| t.id.as_ref()),
        Some("b")
    );
}

/// G2：异常标签（+30dB）因子封顶 4.0（clamp ±12dB）。
#[tokio::test]
// 锁仅用于串行化改 env 的测试；引擎任务从不获取该锁，无死锁风险。
#[allow(clippy::await_holding_lock)]
async fn replaygain_clamps_extreme_values() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a")]]);
    resolver
        .replaygain
        .lock()
        .unwrap()
        .push((TrackId::new("a"), 30.0));
    let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    // 用户音量 0.2：0.2×4.0=0.8（<1.0 不 clamp）——证明因子确实封顶 4.0
    //（未封顶则 0.2×31.6=6.3 → clamp 1.0，断言可区分）。
    handle
        .cmd(Request::Command(PlayerCommand::SetVolume(0.2)))
        .await
        .unwrap();
    wait_idle().await;
    let vol = handle.state_rx.borrow().playback.volume;
    assert!((vol - 0.8).abs() < 1e-9, "+30dB 因子应封顶 4.0: {vol}");
}

/// G2：配置 `[audio] replaygain=false` 时不做补偿（隔离 XDG_CONFIG_HOME）。
#[tokio::test]
// 锁仅用于串行化改 env 的测试；引擎任务从不获取该锁，无死锁风险。
#[allow(clippy::await_holding_lock)]
async fn replaygain_disabled_by_config() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("hmp");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("config.toml"), "[audio]\nreplaygain = false\n").unwrap();
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
    }
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a")]]);
    resolver
        .replaygain
        .lock()
        .unwrap()
        .push((TrackId::new("a"), 6.0)); // 有标签，但配置关闭 → 不补偿
    let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    let vol = handle.state_rx.borrow().playback.volume;
    assert!(
        (vol - 1.0).abs() < 1e-9,
        "replaygain=false 时不应补偿: {vol}"
    );
    unsafe {
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}

/// 打磨：DaemonState 携带当前曲 RG 增益（CLI status 展示用）。
#[tokio::test]
// 锁仅用于串行化改 env 的测试；引擎任务从不获取该锁，无死锁风险。
#[allow(clippy::await_holding_lock)]
async fn state_exposes_replaygain_db() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
    resolver
        .replaygain
        .lock()
        .unwrap()
        .push((TrackId::new("a"), -6.5));
    let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    assert_eq!(
        handle.state_rx.borrow().replaygain_db,
        Some(-6.5),
        "当前曲应携带 RG 标签 dB"
    );
    // Next 到无 RG 曲目 → None。
    handle
        .cmd(Request::Command(PlayerCommand::Next))
        .await
        .unwrap();
    wait_idle().await;
    assert_eq!(
        handle.state_rx.borrow().replaygain_db,
        None,
        "无 RG 曲目应为 None"
    );
}

/// 清空进入 Idle 时 RG 残留必须清零（QueueClear --all / Remove 清空到空）：
/// status 不再显示已消失曲目的增益，此后 SetVolume 不被旧增益污染。
#[tokio::test]
// 锁仅用于串行化改 env 的测试；引擎任务从不获取该锁，无死锁风险。
#[allow(clippy::await_holding_lock)]
async fn idle_transitions_clear_replaygain() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    // QueueClear{all}：清空后 replaygain_db = None，SetVolume 无补偿。
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a")]]);
    resolver
        .replaygain
        .lock()
        .unwrap()
        .push((TrackId::new("a"), 6.0));
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    handle.cmd(Request::QueueClear { all: true }).await.unwrap();
    wait_idle().await;
    assert_eq!(
        handle.state_rx.borrow().replaygain_db,
        None,
        "清空队列后不应残留上一曲增益"
    );
    handle
        .cmd(Request::Command(PlayerCommand::SetVolume(0.5)))
        .await
        .unwrap();
    wait_idle().await;
    let vol = handle.state_rx.borrow().playback.volume;
    assert!(
        (vol - 0.5).abs() < 1e-9,
        "Idle 后 SetVolume 无 RG 补偿: {vol}"
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
        driver.load_uris(),
        vec!["fake://a", "fake://b", "fake://c", "fake://b"]
    );
}

/// 里程碑 G：Previous 曲首语义——position > 3s 只回曲首，不换曲。
#[tokio::test]
async fn previous_restarts_track_when_past_three_seconds() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a")], vec![TrackId::new("b")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    // 位置推进到 >3s。
    driver
        .state_tx
        .send_modify(|s| s.position = std::time::Duration::from_secs(12));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let loads_before = driver.loads.lock().unwrap().len();
    handle
        .cmd(Request::Command(PlayerCommand::Previous))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    // 不换曲：无新 load。
    assert_eq!(
        driver.loads.lock().unwrap().len(),
        loads_before,
        ">3s 时 Previous 不应换曲"
    );
    // 收到 Seek(0)。
    let seeks: Vec<PlayerCommand> = driver
        .commands
        .lock()
        .unwrap()
        .iter()
        .filter(|c| matches!(c, PlayerCommand::Seek(_)))
        .cloned()
        .collect();
    assert!(
        seeks
            .iter()
            .any(|c| matches!(c, PlayerCommand::Seek(p) if p.is_zero())),
        "应 Seek(0) 回曲首: {seeks:?}"
    );
    // 队列/当前曲未变。
    assert_eq!(
        handle
            .state_rx
            .borrow()
            .playback
            .current
            .as_ref()
            .map(|t| t.id.as_ref()),
        Some("a")
    );
}

/// 里程碑 G：position ≤ 3s → 正常换上一首。
#[tokio::test]
async fn previous_switches_track_when_within_three_seconds() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
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
    // 位置显式归零（≤3s）→ Previous 换回 a。
    driver
        .state_tx
        .send_modify(|s| s.position = std::time::Duration::ZERO);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let loads_before = driver.loads.lock().unwrap().len();
    handle
        .cmd(Request::Command(PlayerCommand::Previous))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        driver.loads.lock().unwrap().len() > loads_before,
        "≤3s 时 Previous 应换曲"
    );
    assert_eq!(
        handle
            .state_rx
            .borrow()
            .playback
            .current
            .as_ref()
            .map(|t| t.id.as_ref()),
        Some("a")
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
    // 带 RG：播完进入 Idle 时增益应随曲目清空（打磨 review）。
    resolver
        .replaygain
        .lock()
        .unwrap()
        .push((TrackId::new("a"), -6.5));
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    assert_eq!(handle.state_rx.borrow().replaygain_db, Some(-6.5));
    driver.emit(PlayerEvent::PlaybackEnded { load_gen: 1 }); // 当前代（首载 gen=1）
    wait_idle().await;
    assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
    assert_eq!(handle.state_rx.borrow().phase, hmp_core::EnginePhase::Idle);
    assert_eq!(
        handle.state_rx.borrow().replaygain_db,
        None,
        "Idle 后应清空 RG 增益"
    );
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
    let (handle, _st) = start_engine_with_library(driver.clone(), resolver, library.clone()).await;

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

/// 装载应用有延迟的驱动（模拟真实 Rodio 异步管道：load() 返回后
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
        None,
        std::time::Duration::from_secs(5),
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
        None,
        std::time::Duration::from_secs(5),
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

/// `queue play-at N`（AUDIT §8.8）：跳到第 N 曲播放，**队列不被单曲替换**。
#[tokio::test]
async fn queue_play_at_jumps_without_replacing_queue() {
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

    handle.cmd(Request::QueuePlayAt(2)).await.unwrap(); // 点第 3 曲
    wait_idle().await;
    let st = handle.state_rx.borrow();
    assert_eq!(
        handle.queue_rx.borrow().tracks,
        vec![TrackId::new("a"), TrackId::new("b"), TrackId::new("c")],
        "跳播不得替换队列（旧实现退化 Play(单曲) 的回归）"
    );
    assert_eq!(st.queue.current, Some(2));
    assert_eq!(driver.load_uris(), vec!["fake://a", "fake://c"]);
}

/// 跳播目标装载失败 → 回滚游标，原曲继续（事务语义）。
#[tokio::test]
async fn queue_play_at_rolls_back_on_load_failure() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = PartialFailResolver::new(
        vec![vec![
            TrackId::new("a"),
            TrackId::new("b"),
            TrackId::new("c"),
        ]],
        vec![TrackId::new("c")],
    );
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;

    handle.cmd(Request::QueuePlayAt(2)).await.unwrap();
    wait_idle().await;
    let st = handle.state_rx.borrow();
    assert_eq!(st.queue.current, Some(0), "失败回滚：游标留在原曲");
    assert_eq!(driver.load_uris(), vec!["fake://a"], "失败目标不得加载");
    assert!(st.last_error.is_some(), "跳播失败详情应进入复合状态");
}

/// 越界跳播 → 发布错误、不动队列；点当前曲 = 确保播放（不重载）。
#[tokio::test]
async fn queue_play_at_out_of_range_and_current_semantics() {
    let (driver, _sr, _er) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
    let (handle, _st) = start_engine(driver.clone(), resolver).await;
    handle
        .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
        .await
        .unwrap();
    wait_idle().await;
    assert_eq!(driver.load_uris(), vec!["fake://a"]);

    handle.cmd(Request::QueuePlayAt(5)).await.unwrap();
    wait_idle().await;
    assert!(
        handle.state_rx.borrow().last_error.is_some(),
        "越界索引应发布错误"
    );
    assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
    assert_eq!(driver.load_uris().len(), 1, "越界不触发装载");

    let plays_before = driver.plays.load(std::sync::atomic::Ordering::SeqCst);
    handle.cmd(Request::QueuePlayAt(0)).await.unwrap(); // 点当前曲
    wait_idle().await;
    assert_eq!(driver.load_uris().len(), 1, "点当前曲不重载");
    assert_eq!(
        driver.plays.load(std::sync::atomic::Ordering::SeqCst),
        plays_before + 1,
        "点当前曲应确保播放（play() 恰好一次）"
    );
}

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
    let (handle, _st) = start_engine_with_library(driver.clone(), resolver, library.clone()).await;
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
    let (handle, _st) = start_engine_with_library(driver.clone(), resolver, library.clone()).await;
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
    let (handle, _st) = start_engine_with_library(driver.clone(), resolver, library.clone()).await;
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
    let (handle, _st) = start_engine_with_library(driver.clone(), resolver, library.clone()).await;
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
    let (handle, _st) = start_engine_with_library(driver.clone(), resolver, library.clone()).await;
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
        None,
        std::time::Duration::from_secs(5),
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
        None,
        std::time::Duration::from_secs(5),
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

// ---- 会话持久化（里程碑 D） ----

#[test]
fn session_file_roundtrips() {
    let mut q = hmp_core::QueueCore::new();
    q.append(vec![TrackId::new("qq:a"), TrackId::new("local:/x.mp3")]);
    q.set_current(1);
    q.set_loop_mode(LoopMode::List);
    q.set_shuffle(true);
    let f = SessionFile {
        queue: q.save_state(),
        volume: 0.42,
        position_ms: 12_345,
    };
    let json = serde_json::to_string(&f).unwrap();
    let back: SessionFile = serde_json::from_str(&json).unwrap();
    assert_eq!(back.queue.tracks, f.queue.tracks);
    assert_eq!(back.queue.order, f.queue.order);
    assert_eq!(back.queue.cursor, 1);
    assert!(back.queue.has_current);
    assert_eq!(back.queue.loop_mode, LoopMode::List);
    assert!(back.queue.shuffle);
    assert_eq!(back.volume, 0.42);
    assert_eq!(back.position_ms, 12_345);
}

#[test]
fn session_file_missing_is_none() {
    // 无状态文件 → 恢复为 None（首次启动路径）。
    assert!(
        read_session_file("/nonexistent/hmp-test/no-such.json")
            .unwrap()
            .is_none()
    );
}

#[test]
fn session_file_corrupt_is_none() {
    // 损坏文件不 panic，视为无会话。
    let dir = std::env::temp_dir().join(format!("hmp-session-corrupt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("playback_state.json");
    std::fs::write(&p, "{ not valid json !!!").unwrap();
    let r = read_session_file(&p);
    assert!(r.is_ok());
    assert!(r.unwrap().is_none());
}

/// 会话恢复集成：重启后队列/音量/位置恢复，不自动播放，Play 后续播。
#[tokio::test]
async fn session_restores_after_restart() {
    let dir = std::env::temp_dir().join(format!("hmp-session-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sp = dir.join("playback_state.json");
    // 第一代引擎：构造队列、设音量、推进位置后"退出"（写盘发生在前台命令+节流）。
    let (driver, _, _) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("qq:r1")]]);
    let h1 = PlaybackEngine::start_with_options(
        driver.clone(),
        resolver,
        Arc::new(|| true),
        None,
        std::time::Duration::from_secs(5),
        Some(sp.clone()),
        std::time::Duration::from_millis(0), // 节流 0 → 每次 publish 都写
    );
    h1.cmd(Request::Command(PlayerCommand::SetVolume(0.37)))
        .await
        .unwrap();
    h1.cmd(Request::Play(PlayRequest::Track(TrackId::new("qq:r1"))))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    h1.cmd(Request::Command(PlayerCommand::SetShuffle(true)))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(sp.exists(), "会话文件应已写入");
    // 第二代引擎：同路径恢复。
    let (driver2, _, _) = FakeDriver::new();
    let resolver2 = FakeResolver::new(vec![vec![TrackId::new("qq:r1")]]);
    let h2 = PlaybackEngine::start_with_options(
        driver2.clone(),
        resolver2,
        Arc::new(|| true),
        None,
        std::time::Duration::from_secs(5),
        Some(sp.clone()),
        std::time::Duration::from_secs(5),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let st = h2.state_rx.borrow().clone();
    // 恢复不自动播放。
    assert_ne!(st.playback.status, PlaybackStatus::Playing);
    // 队列已恢复（含洗牌开关——QueueState 整体还原）。
    assert_eq!(h2.queue_rx.borrow().tracks, vec![TrackId::new("qq:r1")]);
    assert!(h2.queue_rx.borrow().shuffle);
    // 音量已恢复（start 时 driver.set_volume 已调用）。
    assert_eq!(st.playback.volume, 0.37);
    // Play 后从保存位置续播（driver 收到 Seek）。
    h2.cmd(Request::Play(PlayRequest::Track(TrackId::new("qq:r1"))))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let seeks: Vec<PlayerCommand> = driver2
        .commands
        .lock()
        .unwrap()
        .iter()
        .filter(|c| matches!(c, PlayerCommand::Seek(_)))
        .cloned()
        .collect();
    assert!(!seeks.is_empty(), "恢复后首次 Play 应发出 Seek 续播");
}

/// 节流：播放中位置推进不触发写盘；暂停后位置变化立即写。
#[tokio::test]
async fn position_persist_is_throttled() {
    let dir = std::env::temp_dir().join(format!("hmp-session-throttle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sp = dir.join("playback_state.json");
    let (driver, _, _) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("qq:t1")]]);
    let h = PlaybackEngine::start_with_options(
        driver.clone(),
        resolver,
        Arc::new(|| true),
        None,
        std::time::Duration::from_secs(5),
        Some(sp.clone()),
        std::time::Duration::from_secs(5), // 长节流
    );
    h.cmd(Request::Play(PlayRequest::Track(TrackId::new("qq:t1"))))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mtime1 = std::fs::metadata(&sp).unwrap().modified().unwrap();
    // 播放中推进位置（send_modify 触发 state_rx.changed → publish）。
    driver
        .state_tx
        .send_modify(|s| s.position = std::time::Duration::from_secs(30));
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mtime2 = std::fs::metadata(&sp).unwrap().modified().unwrap();
    assert_eq!(mtime1, mtime2, "播放中节流期内 position 推进不应触发写盘");
    // 暂停（FakeDriver 空实现，用 set_status 模拟状态翻转→立即写盘）后
    // 再推进位置 → 非 playing 强制写。
    driver.set_status(hmp_core::PlaybackStatus::Paused);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    driver
        .state_tx
        .send_modify(|s| s.position = std::time::Duration::from_secs(31));
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mtime3 = std::fs::metadata(&sp).unwrap().modified().unwrap();
    assert_ne!(
        mtime2, mtime3,
        "暂停后位置变化应立即写盘（非 playing 绕过节流）"
    );
}

/// 写盘失败不 panic、不阻断播放。
#[tokio::test]
async fn session_persist_failure_is_swallowed() {
    let sp = std::path::PathBuf::from("/nonexistent-dir-hmp/playback_state.json");
    let (driver, _, _) = FakeDriver::new();
    let resolver = FakeResolver::new(vec![vec![TrackId::new("qq:f1")]]);
    let h = PlaybackEngine::start_with_options(
        driver.clone(),
        resolver,
        Arc::new(|| true),
        None,
        std::time::Duration::from_secs(5),
        Some(sp),
        std::time::Duration::from_millis(0),
    );
    h.cmd(Request::Play(PlayRequest::Track(TrackId::new("qq:f1"))))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let st = h.state_rx.borrow().clone();
    assert_eq!(st.playback.status, PlaybackStatus::Playing);
}
