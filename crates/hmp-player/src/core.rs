//! Rodio-backed player state machine.

use std::fs::File;
use std::sync::Arc;
use std::time::Duration;

use hmp_core::{
    HmpError, LoadRequest, MediaStream, PlaybackState, PlaybackStatus, PlayerCommand, PlayerEvent,
};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source, StreamError};
use tokio::sync::{broadcast, mpsc, watch};

use crate::source::{MediaLocation, parse_uri};

/// rodio 的 `DecoderBuilder` 要求 reader `Send + Sync`，而 hmp-core 契约的
/// [`MediaStream`] 只有 `Send`。Read/Seek 都是 `&mut self` 独占访问，decoder
/// 是唯一消费者（无并发锁竞争），用 `Mutex` 桥接 `Sync`——无 unsafe。
struct SyncStream(std::sync::Mutex<Box<dyn MediaStream>>);

impl SyncStream {
    fn new(stream: Box<dyn MediaStream>) -> Self {
        Self(std::sync::Mutex::new(stream))
    }
}

impl std::io::Read for SyncStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // 锁中毒（reader 读中途 panic）不致命：继续用内部 reader。
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .read(buf)
    }
}

impl std::io::Seek for SyncStream {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .seek(pos)
    }
}

enum LoadCommand {
    Load(Box<LoadRequest>),
    Shutdown,
}

/// 装载任务完成消息（oneshot 回传 drive 循环）。
///
/// 被更新的装载/Stop/Shutdown 取代时接收端被 drop，发送端 `send` 失败
/// 即静默作废——取代语义的一部分，不是错误。
struct LoadCompletion {
    request: Box<LoadRequest>,
    result: Result<Option<Duration>, HmpError>,
}

/// Open the platform-appropriate output stream (safe variant).
///
/// Candidate devices come from [`collect_output_candidates`] (platform
/// policy); each is tried in preference order and the first stream that
/// opens wins. Never fatal: an empty candidate list or all-failed opens
/// surfaces as `Err` and the caller falls back to the silent sink.
pub fn open_default_output() -> Result<OutputStream, StreamError> {
    use rodio::cpal::traits::DeviceTrait;

    let mut candidates = collect_output_candidates();
    candidates.sort_by_key(|&(preference, _)| preference);

    let mut last_error = StreamError::NoDevice;
    for (_, device) in candidates {
        let name = device.name().unwrap_or_else(|_| "<unnamed>".into());
        let builder = match OutputStreamBuilder::from_device(device) {
            Ok(builder) => builder,
            Err(error) => {
                tracing::warn!(%name, %error, "audio device rejected builder config, trying next");
                last_error = error;
                continue;
            }
        };
        match builder.open_stream() {
            Ok(stream) => {
                tracing::info!(%name, "audio output device opened");
                return Ok(stream);
            }
            Err(error) => {
                tracing::warn!(%name, %error, "audio device open failed, trying next");
                last_error = error;
            }
        }
    }
    Err(last_error)
}

/// Candidate output devices with preference order (lower = tried first).
type OutputCandidates = Vec<(usize, rodio::cpal::Device)>;

/// Unix candidate policy: server-routed PCMs only.
///
/// Never enumerates-and-opens hardware devices: on failure rodio 0.21's
/// `OutputStreamBuilder::open_default_stream()` falls back to opening EVERY
/// output device, and cpal's ALSA enumerator really opens `plughw:N`
/// passthrough PCMs per card (`cpal-0.16 host/alsa/enumerate.rs`, USB DACs
/// usually being card 0). Opening a passthrough PCM takes the hardware
/// exclusively behind the system audio server's back, which starves
/// PipeWire/PulseAudio and makes the device fail/disappear from system audio
/// controls (2026-09-07 DAWN PRO2 incident).
///
/// Instead we walk the server-routed PCM candidates in preference order
/// (`"default"` → `"pipewire"` → `"pulse"`) and open the first that works.
/// The fallback exists because a system's `"default"` PCM is not always
/// server-routed: e.g. a missing `pcm.!default` override leaves ALSA's
/// built-in plug→dmix default, which fails outright when the audio server
/// holds the same card (2026-09-08: dmix could not open the PipeWire-held
/// PRO2 slave, daemon exited at startup, desktop auto-spawn failed → whole
/// UI offline). `pipewire`/`pulse` PCMs route through the server too, so
/// they can never grab hardware; direct hardware PCMs stay refused in any
/// case.
#[cfg(unix)]
fn collect_output_candidates() -> OutputCandidates {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};

    let host = rodio::cpal::default_host();
    let mut candidates: Vec<(usize, rodio::cpal::Device)> = Vec::new();
    let consider = |device: Option<rodio::cpal::Device>,
                    candidates: &mut Vec<(usize, rodio::cpal::Device)>| {
        let Some(device) = device else { return };
        let Ok(name) = device.name() else {
            return;
        };
        let Some(preference) = server_routed_preference(&name) else {
            return;
        };
        if candidates
            .iter()
            .any(|(_, device)| device.name().ok().as_deref() == Some(name.as_str()))
        {
            return;
        }
        candidates.push((preference, device));
    };
    consider(host.default_output_device(), &mut candidates);
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            consider(Some(device), &mut candidates);
        }
    }
    candidates
}

/// Windows candidate policy: WASAPI render endpoints, default first.
///
/// cpal's WASAPI host opens every stream in *shared* mode — samples go
/// through audiodg's mixer graph, never exclusively to the hardware — so the
/// ALSA `plughw` exclusivity incident cannot happen here and no name filter
/// is needed. The default render endpoint is tried first; every other
/// endpoint follows as ordered fallback (default disabled/unplugged, virtual
/// cables like VB-CABLE, HDMI/DP monitor audio hotplug races). The default
/// may appear twice (it also shows up in the enumeration); a second failed
/// open of the same endpoint is harmless and keeps this branch trivial.
///
/// Device names are localized (e.g. "耳机 (DAWN PRO2)", "扬声器 (Realtek(R)
/// Audio)") — do NOT reintroduce literal-name matching here.
#[cfg(windows)]
fn collect_output_candidates() -> OutputCandidates {
    use rodio::cpal::traits::HostTrait;

    let host = rodio::cpal::default_host();
    let mut candidates = Vec::new();
    if let Some(device) = host.default_output_device() {
        candidates.push((0, device));
    }
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            candidates.push((1, device));
        }
    }
    candidates
}

/// Server-routed PCM policy (Unix only): preference order of PCM names that
/// always route through the system audio server. Direct hardware PCMs
/// (`hw:*`, `plughw:*`, `front:*`, `surround*:*`), OSS (`oss`) and JACK
/// (`jack`) are refused so the player can never grab exclusive hardware
/// behind PipeWire/PulseAudio.
#[cfg(unix)]
fn server_routed_preference(name: &str) -> Option<usize> {
    ["default", "pipewire", "pulse"]
        .into_iter()
        .position(|candidate| candidate == name)
}

/// Audio output plus the channels used by all application adapters.
pub struct PlayerCore {
    cmd_tx: mpsc::UnboundedSender<PlayerCommand>,
    load_tx: mpsc::UnboundedSender<LoadCommand>,
    state_rx: watch::Receiver<PlaybackState>,
    events_rx: broadcast::Receiver<PlayerEvent>,
    // Rodio stops the device stream when this guard is dropped.
    _output: Option<OutputStream>,
}

impl PlayerCore {
    /// Open the platform default output device.
    ///
    /// 无音频输出可用（无设备/无音频服务的 VM、CI、带声卡被独占的会话）时
    /// 不再致命：回退 rodio 无设备静默 sink + 泵线程——队列时钟照常推进
    /// （EOS/自动切歌/位置上报/SMTC 照常），仅无声。daemon 必须可启动，
    /// 是否可出声不应决定 IPC 服务存活（桌面自动拉起依赖它）。
    pub fn new() -> Result<Self, HmpError> {
        match open_default_output() {
            Ok(output) => {
                let sink = Sink::connect_new(output.mixer());
                Ok(Self::from_sink(sink, Some(output)))
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "no audio output available; using silent output (clock keeps running)"
                );
                let (sink, mut output) = Sink::new();
                std::thread::spawn(move || {
                    loop {
                        let samples_per_tick =
                            (output.sample_rate() as usize * output.channels() as usize / 100)
                                .max(1);
                        for _ in 0..samples_per_tick {
                            if output.next().is_none() {
                                return;
                            }
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                });
                Ok(Self::from_sink(sink, None))
            }
        }
    }

    fn from_sink(sink: Sink, output: Option<OutputStream>) -> Self {
        let sink = Arc::new(sink);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (load_tx, load_rx) = mpsc::unbounded_channel();
        let (state_tx, state_rx) = watch::channel(PlaybackState::default());
        let (events_tx, events_rx) = broadcast::channel(64);
        tokio::spawn(drive(sink, cmd_rx, load_rx, state_tx, events_tx));
        Self {
            cmd_tx,
            load_tx,
            state_rx,
            events_rx,
            _output: output,
        }
    }

    /// Construct a deterministic sink for unit tests without an audio device.
    #[cfg(test)]
    pub(crate) fn new_silent_for_test() -> Self {
        let (sink, mut output) = Sink::new();
        std::thread::spawn(move || {
            loop {
                let samples_per_tick =
                    (output.sample_rate() as usize * output.channels() as usize / 100).max(1);
                for _ in 0..samples_per_tick {
                    if output.next().is_none() {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Self::from_sink(sink, None)
    }

    pub fn load(&self, request: LoadRequest) {
        let _ = self.load_tx.send(LoadCommand::Load(Box::new(request)));
    }

    pub fn play(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Play);
    }

    pub fn pause(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Pause);
    }

    pub fn stop(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Stop);
    }

    pub fn seek(&self, position: Duration) {
        let _ = self.cmd_tx.send(PlayerCommand::Seek(position));
    }

    pub fn set_volume(&self, volume: f64) {
        let _ = self.cmd_tx.send(PlayerCommand::SetVolume(volume));
    }

    pub fn shutdown(&self) {
        let _ = self.load_tx.send(LoadCommand::Shutdown);
    }

    pub fn command_sender(&self) -> mpsc::UnboundedSender<PlayerCommand> {
        self.cmd_tx.clone()
    }

    pub fn subscribe_state(&self) -> watch::Receiver<PlaybackState> {
        self.state_rx.clone()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent> {
        self.events_rx.resubscribe()
    }
}

async fn append_location(
    sink: &Sink,
    location: MediaLocation,
) -> Result<Option<Duration>, HmpError> {
    // 格式探测（decoder 构建）是**阻塞读**（MediaStream 的 Read 在数据
    // 饥饿时阻塞等源的后台预取），必须在 `spawn_blocking` 上做：若内联在
    // runtime worker 上，worker 被读挂起，而源的后台预取生产任务
    // （tokio::spawn）可能正落在本 worker 的 LIFO 槽（不可被其他 worker
    // 偷取）→ 生产任务永不调度、探测永远等不到字节，装载死锁
    // （2026-09-29 QQ 远端音频 5s 超时事故的第二个根因；stream-download
    // 时代同型死锁，进程内源沿用同一条纪律）。
    // 构建好的 decoder 回传后再 append：装载被取代时本任务的结果经 oneshot
    // 丢弃，decoder 随之释放，不会追加过期音源。
    let decoder: Box<dyn Source<Item = f32> + Send> = match location {
        MediaLocation::File(path) => {
            spawn_decode(move || {
                let file = File::open(&path).map_err(|error| {
                    HmpError::Playback(format!("open audio file {}: {error}", path.display()))
                })?;
                Decoder::try_from(file)
                    .map(|decoder| Box::new(decoder) as Box<dyn Source<Item = f32> + Send>)
                    .map_err(|error| {
                        HmpError::Playback(format!("decode audio file {}: {error}", path.display()))
                    })
            })
            .await?
        }
        MediaLocation::Stream(source) => {
            // open 在 tokio 上下文中调用（源内部 spawn 后台预取任务，
            // 挂在当前 runtime 上）；reader 的 Read 仅在数据饥饿时阻塞。
            let reader = source
                .open()
                .map_err(|error| HmpError::Playback(format!("open media stream: {error}")))?;
            let builder = Decoder::builder().with_data(SyncStream::new(reader));
            spawn_decode(move || {
                builder
                    .build()
                    .map(|decoder| Box::new(decoder) as Box<dyn Source<Item = f32> + Send>)
                    .map_err(|error| HmpError::Playback(format!("decode media stream: {error}")))
            })
            .await?
        }
    };
    let duration = decoder.total_duration();
    sink.append(decoder);
    Ok(duration)
}

/// 在阻塞线程池上构建 decoder（见 `append_location` 注释）。
async fn spawn_decode<F>(build: F) -> Result<Box<dyn Source<Item = f32> + Send>, HmpError>
where
    F: FnOnce() -> Result<Box<dyn Source<Item = f32> + Send>, HmpError> + Send + 'static,
{
    tokio::task::spawn_blocking(build)
        .await
        .map_err(|error| HmpError::Playback(format!("decode task aborted: {error}")))?
}

/// 装载任务体：打开源并解码、追加进 sink。**只做阻塞在自身 future 里的
/// 取流/解码工作**，状态转换一律由 drive 循环在收到 [`LoadCompletion`] 后
/// 执行（任务可能被更新的装载/Stop/Shutdown abort，不得自行发布状态）。
async fn perform_load(sink: Arc<Sink>, request: Box<LoadRequest>) -> LoadCompletion {
    // stream 优先于 uri：远端曲目（明文/加密）一律经进程内源直连；
    // uri 仅元数据/日志。无 stream 才按 file:// 解析本地文件。
    let location = match request.stream.as_ref() {
        Some(source) => MediaLocation::Stream(Arc::clone(source)),
        None => match parse_uri(&request.uri) {
            Ok(location) => location,
            Err(error) => {
                return LoadCompletion {
                    request,
                    result: Err(error),
                }
            }
        },
    };
    sink.clear();
    let result = append_location(&sink, location).await;
    LoadCompletion { request, result }
}

async fn drive(
    sink: Arc<Sink>,
    mut cmd_rx: mpsc::UnboundedReceiver<PlayerCommand>,
    mut load_rx: mpsc::UnboundedReceiver<LoadCommand>,
    state_tx: watch::Sender<PlaybackState>,
    events_tx: broadcast::Sender<PlayerEvent>,
) {
    let mut state = PlaybackState::default();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 派发中的装载（取流/解码在独立任务；命令循环永不为之阻塞——否则一次
    // 远端取流挂起就冻结全部命令处理，驱动永久僵尸，2026-09-29 事故）。
    let mut load_abort: Option<tokio::task::AbortHandle> = None;
    let mut load_done: Option<tokio::sync::oneshot::Receiver<LoadCompletion>> = None;

    loop {
        tokio::select! {
            command = cmd_rx.recv() => {
                let Some(command) = command else { continue };
                match command {
                    PlayerCommand::Play => {
                        if state.current.is_some() {
                            sink.play();
                            state.status = PlaybackStatus::Playing;
                        }
                    }
                    PlayerCommand::Pause => {
                        sink.pause();
                        if state.current.is_some() {
                            state.status = PlaybackStatus::Paused;
                        }
                    }
                    PlayerCommand::TogglePlay => {
                        if state.status == PlaybackStatus::Playing {
                            sink.pause();
                            state.status = PlaybackStatus::Paused;
                        } else if state.current.is_some() {
                            sink.play();
                            state.status = PlaybackStatus::Playing;
                        }
                    }
                    PlayerCommand::Stop => {
                        // 未完成装载一并取消：否则晚到的装载完成会在 Stop 后
                        // 「复活」为 Playing（用户已明确要求停止）。
                        if let Some(abort) = load_abort.take() {
                            abort.abort();
                        }
                        drop(load_done.take());
                        sink.stop();
                        state.status = PlaybackStatus::Stopped;
                        state.position = Duration::ZERO;
                    }
                    PlayerCommand::Seek(position) => {
                        match sink.try_seek(position) {
                            Ok(()) => state.position = position,
                            Err(error) => {
                                // seek 失败不宣告播放器故障：曲目继续从原位置播，
                                // 仅发错误事件。置 Error 会冻结进度（ticker 只在
                                // Playing/Paused 推进）并让 CLI 把已成功的装载误判
                                // 为失败（2026-09-29 恢复续播 seek 未缓冲区间时）。
                                let error = HmpError::Playback(format!("seek audio stream: {error}"));
                                tracing::warn!(%error, "seek failed; keeping playback state");
                                let _ = events_tx.send(PlayerEvent::Error {
                                    load_gen: state.load_gen,
                                    error,
                                });
                            }
                        }
                    }
                    PlayerCommand::SetVolume(volume) => {
                        state.volume = volume.clamp(0.0, 1.0);
                        sink.set_volume(state.volume as f32);
                    }
                    PlayerCommand::SetLoopMode(mode) => state.loop_mode = mode,
                    PlayerCommand::SetShuffle(shuffle) => state.shuffle = shuffle,
                    PlayerCommand::Next
                    | PlayerCommand::Previous
                    | PlayerCommand::LoadAndPlay(_) => {}
                }
                let _ = state_tx.send(state.clone());
            }
            load = load_rx.recv() => {
                let Some(load) = load else { continue };
                match load {
                    LoadCommand::Shutdown => {
                        if let Some(abort) = load_abort.take() {
                            abort.abort();
                        }
                        break;
                    }
                    LoadCommand::Load(request) => {
                        // 取代进行中的装载：abort 未完成任务、作废旧结果
                        // （drop 接收端，任务侧 send 失败即静默）。新旧任务对
                        // sink 的 clear 竞口窗口为一个调度间隔，随后任务启动
                        // 时的 clear 会清掉任何晚追加的旧解码器。
                        if let Some(abort) = load_abort.take() {
                            abort.abort();
                        }
                        drop(load_done.take());
                        state.status = PlaybackStatus::Loading;
                        state.buffering = Some(0.0);
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::BufferingChanged(Some(0.0)));
                        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
                        let task_sink = Arc::clone(&sink);
                        let abort = tokio::spawn(async move {
                            let completion = perform_load(task_sink, request).await;
                            let _ = done_tx.send(completion);
                        })
                        .abort_handle();
                        load_abort = Some(abort);
                        load_done = Some(done_rx);
                    }
                }
            }
            completion = async {
                match load_done.as_mut() {
                    Some(done_rx) => done_rx.await.ok(),
                    None => std::future::pending().await,
                }
            } => {
                load_done = None;
                load_abort = None;
                // None = 被取代/中止（发送端失败），非错误。
                let Some(completion) = completion else { continue };
                let request = completion.request;
                match completion.result {
                    Ok(duration) => {
                        state.current = Some(request.track);
                        state.actual_quality = Some(request.quality);
                        state.load_gen = request.load_gen;
                        state.position = Duration::ZERO;
                        state.duration = duration;
                        state.can_seek = true;
                        state.buffering = None;
                        sink.play();
                        state.status = PlaybackStatus::Playing;
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::BufferingChanged(None));
                        let _ = events_tx.send(PlayerEvent::TrackChanged);
                    }
                    Err(error) => {
                        state.status = PlaybackStatus::Error;
                        state.buffering = None;
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::Error {
                            load_gen: request.load_gen,
                            error,
                        });
                    }
                }
            }
            _ = ticker.tick() => {
                if state.current.is_some() && matches!(state.status, PlaybackStatus::Playing | PlaybackStatus::Paused) {
                    state.position = sink.get_pos();
                    if state.status == PlaybackStatus::Playing && sink.empty() {
                        state.status = PlaybackStatus::Ended;
                        if let Some(duration) = state.duration {
                            state.position = duration;
                        }
                        let _ = events_tx.send(PlayerEvent::PlaybackEnded {
                            load_gen: state.load_gen,
                        });
                    }
                    let _ = state_tx.send(state.clone());
                }
            }
        }
    }
    sink.stop();
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    /// 输出设备安全策略：只允许经系统音频服务器路由的 PCM。回归守护：
    /// 防止再次引入"枚举/直通设备 fallback"而独占硬件
    /// （2026-09-07 DAWN PRO2 被抢事件）。Unix 策略（Windows 走 WASAPI
    /// 共享模式全接纳，见 `collect_output_candidates` 平台分叉）。
    #[cfg(unix)]
    #[test]
    fn output_policy_accepts_only_server_routed_pcms() {
        assert_eq!(server_routed_preference("default"), Some(0));
        assert_eq!(server_routed_preference("pipewire"), Some(1));
        assert_eq!(server_routed_preference("pulse"), Some(2));
    }

    #[cfg(unix)]
    #[test]
    fn output_policy_refuses_hardware_passthrough_and_plugins() {
        for name in [
            "hw:0",
            "plughw:0",
            "plughw:2",
            "front:0",
            "front:CARD=PRO2,DEV=0",
            "surround51:0",
            "jack",
            "oss",
            "",
        ] {
            assert_eq!(
                server_routed_preference(name),
                None,
                "{name} must be refused"
            );
        }
    }

    /// 候选序恒为 default → pipewire → pulse：与枚举顺序无关，
    /// 任何系统上都优先语义上的平台默认 PCM。
    #[cfg(unix)]
    #[test]
    fn candidate_preference_orders_default_first() {
        let mut prefs: Vec<Option<usize>> = vec![
            server_routed_preference("pulse"),
            server_routed_preference("default"),
            server_routed_preference("pipewire"),
        ];
        prefs.sort_by_key(|p| p.unwrap_or(usize::MAX));
        assert_eq!(prefs, vec![Some(0), Some(1), Some(2)]);
    }
}
