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
struct SyncStream {
    inner: std::sync::Mutex<Box<dyn MediaStream>>,
    /// 中途读错误闩（见 [`SourceErrorLatch`]）。
    latch: SourceErrorLatch,
}

/// 源读错误闩（AUDIT §16 开放项：中途读错误被 rodio 视为 EOS → 静默跳歌）。
///
/// rodio/symphonia 把 reader 的 IO 错误当源终止（rodio-0.21.1
/// `decoder/symphonia.rs::next` 的 `Err(_) => return None`），`sink.empty()`
/// 无法区分自然播完与错误终止。reader 包装层在 `Read` 返回 `Err` 时置闩
/// （首错粘滞），drive ticker 在判 EOS 前查询：闩有错 → 显式
/// `PlayerEvent::Error` + `PlaybackStatus::Error`，而非 Ended——引擎侧
/// 不再静默跳歌。**只在 Read 置闩**：seek 错误由 `try_seek` 结果自带
/// 报告（保持播放态的既有语义），置闩反而会把「seek 失败后自然播完」
/// 误判为中途错误。
type SourceErrorLatch = std::sync::Arc<std::sync::Mutex<Option<String>>>;

fn new_error_latch() -> SourceErrorLatch {
    std::sync::Arc::new(std::sync::Mutex::new(None))
}

/// 置闩（首错粘滞：只记第一个错误，避免高频读错误反复改写）。
fn latch_set(latch: &SourceErrorLatch, context: &str, err: &std::io::Error) {
    let mut g = latch
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if g.is_none() {
        *g = Some(format!("{context}: {err}"));
    }
}

/// 取走闩中错误（消费语义：ticker 判定一次即清）。
fn latch_take(latch: &SourceErrorLatch) -> Option<String> {
    latch
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

impl SyncStream {
    fn new(stream: Box<dyn MediaStream>, latch: SourceErrorLatch) -> Self {
        Self {
            inner: std::sync::Mutex::new(stream),
            latch,
        }
    }
}

impl std::io::Read for SyncStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // 锁中毒（reader 读中途 panic）不致命：继续用内部 reader。
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .read(buf)
            .inspect_err(|e| latch_set(&self.latch, "media stream read", e))
    }
}

impl std::io::Seek for SyncStream {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .seek(pos)
    }
}

/// 本地文件的同款错误闩包装（`Decoder::try_from(file)` 的中途读错同走
/// rodio 静默终止路径；本地盘错误罕见但同病，一并显式化）。
struct LatchedFile {
    file: File,
    latch: SourceErrorLatch,
}

impl std::io::Read for LatchedFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file
            .read(buf)
            .inspect_err(|e| latch_set(&self.latch, "audio file read", e))
    }
}

impl std::io::Seek for LatchedFile {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

enum LoadCommand {
    Load(Box<LoadRequest>),
    Shutdown,
}

/// seek ACK 软期限（§16 开放项 #3）：后台 seek 超过该时长仅告警并解除
/// hold-pause——seek 可能迟到生效（迟到结果仍按代际守卫应用），命令循环
/// 与 UI 不等待。远端窗口外 seek 合法耗时可达数秒，10s 内报态、之外不误杀。
const SEEK_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// 后台 seek 结果（§16 开放项 #3）。双代际守卫：`seek_gen` 判「是否被
/// 更新的 seek / Stop / Shutdown 取代」，`load_gen` 判「装载是否已换代」；
/// 任一过期即丢弃，不污染新状态。错误预格式化为 `String`（`Sink::try_seek`
/// 的错误类型不跨线程语义暴露，仅用于事件消息）。
struct SeekOutcome {
    seek_gen: u64,
    load_gen: u64,
    position: Duration,
    result: Result<(), String>,
}

/// 装载任务完成消息（oneshot 回传 drive 循环）。
///
/// 被更新的装载/Stop/Shutdown 取代时接收端被 drop，发送端 `send` 失败
/// 即静默作废——取代语义的一部分，不是错误。
struct LoadCompletion {
    request: Box<LoadRequest>,
    result: Result<Option<Duration>, HmpError>,
    /// 本次装载 reader 的错误闩（§16：中途读错误显式化；result 为 Err 时
    /// 未使用——装载失败本身已走 completion Err 分支）。
    latch: SourceErrorLatch,
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
) -> Result<(Option<Duration>, SourceErrorLatch), HmpError> {
    // 错误闩随 reader 建立（§16：中途读错误显式化），经 LoadCompletion
    // 回 drive 循环供 ticker 在判 EOS 前查询。
    let latch = new_error_latch();
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
            let latch = latch.clone();
            spawn_decode(move || {
                let file = File::open(&path).map_err(|error| {
                    HmpError::Playback(format!("open audio file {}: {error}", path.display()))
                })?;
                // 对齐 `Decoder::try_from(File)` 的设置（byte_len + seekable
                // 提升seek性能）；包一层错误闩 reader（§16）。
                let byte_len = file
                    .metadata()
                    .map_err(|e| HmpError::Playback(format!("stat audio file: {e}")))?
                    .len();
                Decoder::builder()
                    .with_data(LatchedFile { file, latch })
                    .with_byte_len(byte_len)
                    .build()
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
            let builder = Decoder::builder().with_data(SyncStream::new(reader, latch.clone()));
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
    Ok((duration, latch))
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
                    latch: new_error_latch(),
                };
            }
        },
    };
    sink.clear();
    let (result, latch) = match append_location(&sink, location).await {
        Ok((duration, latch)) => (Ok(duration), latch),
        Err(e) => (Err(e), new_error_latch()),
    };
    LoadCompletion {
        request,
        result,
        latch,
    }
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
    // 当前装载 reader 的错误闩（§16：判 EOS 前查询，中途读错误 → 显式
    // Error 态；装载分派/Stop 清除、completion 换新）。
    let mut current_latch: Option<SourceErrorLatch> = None;
    // §16 开放项 #3：seek 后台化。`Sink::try_seek` 同步等音频线程执行
    // seek（feedback.recv()），远端未缓冲区间的探测读受 CDN 超时钳制
    // （最长 ~30-40s），内联会冻结本循环（全部播放命令排队）。改为
    // spawn_blocking 投递 + 结果回投本通道，循环只登记代际与软期限。
    let mut seek_gen: u64 = 0;
    let mut seek_ack_deadline: Option<tokio::time::Instant> = None;
    // hold-pause：seek 在途期间压住出声（对齐内联时代的无声窗口，并消除
    // 「seek 生效前旧位置出声」/rollback 重载 0 位瞬播）；用户显式播放控制
    // 命令（Play/Pause/TogglePlay/Stop）到达即解除，outcome/超时后恢复。
    let mut seek_hold = false;
    let (seek_tx, mut seek_rx) = mpsc::unbounded_channel::<SeekOutcome>();

    loop {
        tokio::select! {
            command = cmd_rx.recv() => {
                let Some(command) = command else { continue };
                match command {
                    PlayerCommand::Play => {
                        if state.current.is_some() {
                            if state.status == PlaybackStatus::Error {
                                // Error 态点播放 = 用户显式恢复（O1）：清闩，
                                // 已排空的源经 ticker 判 Ended → PlaybackEnded
                                // 续播下一首（干净恢复，不再闪回 Error）。
                                // 全新装载路径（Request::Play）本就重建闩。
                                if let Some(latch) = current_latch.as_ref() {
                                    latch_take(latch);
                                }
                            }
                            // 用户显式播放意图优先于 seek hold（§16 #3）。
                            seek_hold = false;
                            sink.play();
                            state.status = PlaybackStatus::Playing;
                        }
                    }
                    PlayerCommand::Pause => {
                        // 用户显式暂停意图优先于 seek hold（§16 #3）：hold
                        // 解除后迟到的 seek 结果不得自行恢复出声。
                        seek_hold = false;
                        sink.pause();
                        // 装载在途（Loading）时同样记录暂停意图：completion
                        // 分支据此不自动开播（否则曲尾自动切歌/装载期按暂停，
                        // 过一会儿自己响）。首装载无 current 时也须记录；
                        // Stopped 空闲态不受影响。
                        if state.current.is_some() || state.status == PlaybackStatus::Loading {
                            state.status = PlaybackStatus::Paused;
                        }
                    }
                    PlayerCommand::TogglePlay => {
                        if state.status == PlaybackStatus::Playing {
                            seek_hold = false;
                            sink.pause();
                            state.status = PlaybackStatus::Paused;
                        } else if state.current.is_some() {
                            if state.status == PlaybackStatus::Error {
                                // 同 Play：Error 态切换播放 = 显式恢复（O1）。
                                if let Some(latch) = current_latch.as_ref() {
                                    latch_take(latch);
                                }
                            }
                            seek_hold = false;
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
                        // M3（§16 #3）：失效在途 seek——bump 代际后迟到的
                        // seek 结果按守卫丢弃，不污染 Stopped 态位置；hold
                        // 与软期限一并解除（sink 本身已被 stop）。
                        seek_gen += 1;
                        seek_ack_deadline = None;
                        seek_hold = false;
                        sink.stop();
                        current_latch = None;
                        state.status = PlaybackStatus::Stopped;
                        state.position = Duration::ZERO;
                    }
                    PlayerCommand::Seek(position) => {
                        // §16 开放项 #3：try_seek 内联会同步冻结本循环最长
                        // ~30-40s（CDN 超时钳制的探测读）。改为登记代际 +
                        // spawn_blocking 投递；结果经 seek_rx 回投后按守卫
                        // 应用（见 seek_rx 臂）。后发 seek 覆盖 rodio 单槽
                        // SeekOrder，前者按 rodio 语义回 Ok(())——由 seek_gen
                        // 守卫丢弃陈旧结果。
                        seek_gen += 1;
                        let outcome_seek_gen = seek_gen;
                        let load_gen = state.load_gen;
                        // hold-pause：对齐现状听感（内联 seek 期间音频线程
                        // 本就停摆=无声），消除 seek 生效前旧位置出声窗口。
                        if state.status == PlaybackStatus::Playing {
                            sink.pause();
                            seek_hold = true;
                        }
                        seek_ack_deadline = Some(tokio::time::Instant::now() + SEEK_ACK_TIMEOUT);
                        let seek_sink = Arc::clone(&sink);
                        let outcome_tx = seek_tx.clone();
                        tokio::task::spawn_blocking(move || {
                            let result = seek_sink.try_seek(position);
                            // 无界通道上的同步回投（非阻塞；接收端随 drive
                            // 生命周期，驱动退出后 send 失败即静默作废）。
                            let _ = outcome_tx.send(SeekOutcome {
                                seek_gen: outcome_seek_gen,
                                load_gen,
                                position,
                                result: result.map_err(|error| error.to_string()),
                            });
                        });
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
                        // M3（§16 #3）：在途 seek 随退出失效——本循环 break 后
                        // seek_rx 接收端 drop，spawn_blocking 侧回投 send 失败
                        // 即静默作废，无污染面（无需 bump 代际：不再有消费方）。
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
                        // 新装载取代在途 seek（§16 #3）：软期限与 hold 随装载
                        // 作废，迟到结果按 load_gen 守卫丢弃。
                        seek_ack_deadline = None;
                        seek_hold = false;
                        // 旧装载的错误闩随旧装载作废（§16）。
                        current_latch = None;
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
                        current_latch = Some(completion.latch);
                        state.current = Some(request.track);
                        state.actual_quality = Some(request.quality);
                        state.load_gen = request.load_gen;
                        state.position = Duration::ZERO;
                        state.duration = duration;
                        state.can_seek = true;
                        state.buffering = None;
                        // 装载期间用户已暂停（Pause 在 Loading 态置 Paused，
                        // 见命令分支）→ 保持 Paused，不自动开播——暂停意图
                        // 不丢。解码器已 append 且 sink 处于 paused，后续
                        // Play 命令正常恢复。其余到达 completion 的状态只有
                        // Loading/Playing（Stop 会 abort 装载、Ended 只在
                        // Load 分派前瞬时存在）→ 新装载即播放意图，play。
                        if state.status != PlaybackStatus::Paused {
                            sink.play();
                            state.status = PlaybackStatus::Playing;
                        }
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
            outcome = seek_rx.recv() => {
                // 后台 seek 结果回投（§16 开放项 #3）：双守卫过滤后在本地
                // 应用——命令循环从未为 seek 阻塞。
                let Some(outcome) = outcome else { continue };
                if outcome.seek_gen == seek_gen && outcome.load_gen == state.load_gen {
                    seek_ack_deadline = None;
                    if seek_hold {
                        seek_hold = false;
                        if state.status == PlaybackStatus::Playing {
                            sink.play();
                        }
                    }
                    match outcome.result {
                        Ok(()) => {
                            state.position = outcome.position;
                            let _ = state_tx.send(state.clone());
                        }
                        Err(error) => {
                            // seek 失败不宣告播放器故障：曲目继续从原位置播，
                            // 仅发错误事件。置 Error 会冻结进度（ticker 只在
                            // Playing/Paused 推进）并让 CLI 把已成功的装载误判
                            // 为失败（2026-09-29 恢复续播 seek 未缓冲区间时）。
                            let error =
                                HmpError::Playback(format!("seek audio stream: {error}"));
                            tracing::warn!(%error, "seek failed; keeping playback state");
                            let _ = events_tx.send(PlayerEvent::Error {
                                load_gen: state.load_gen,
                                error,
                            });
                        }
                    }
                } else {
                    tracing::debug!(
                        seek_gen = outcome.seek_gen,
                        load_gen = outcome.load_gen,
                        "丢弃过期 seek 结果（被更新的 seek/装载/Stop 取代）"
                    );
                }
            }
            _ = async {
                match seek_ack_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                // ACK 软期限到（§16 #3）：仅告警并解除 hold——seek 可能迟到
                // 生效，迟到结果仍按代际守卫应用。
                tracing::warn!("seek acknowledgement timeout; late result will apply if still current");
                seek_ack_deadline = None;
                if seek_hold {
                    seek_hold = false;
                    if state.status == PlaybackStatus::Playing {
                        sink.play();
                    }
                }
            }
            _ = ticker.tick() => {
                if state.current.is_some() && matches!(state.status, PlaybackStatus::Playing | PlaybackStatus::Paused) {
                    state.position = sink.get_pos();
                    if state.status == PlaybackStatus::Playing && sink.empty() {
                        // 中途读错误判定（§16 开放项）：闩有错 → 显式 Error
                        // 态 + Error 事件（不 Ended、不前移位置、不发
                        // PlaybackEnded——引擎不再静默跳歌）。仅自然排空才
                        // 判曲尾。
                        let latched_error = current_latch.as_ref().and_then(latch_take);
                        if let Some(err) = latched_error {
                            state.status = PlaybackStatus::Error;
                            let error = HmpError::Playback(format!("source read error: {err}"));
                            tracing::warn!(%error, "source read failed mid-playback");
                            let _ = state_tx.send(state.clone());
                            let _ = events_tx.send(PlayerEvent::Error {
                                load_gen: state.load_gen,
                                error,
                            });
                        } else {
                            state.status = PlaybackStatus::Ended;
                            if let Some(duration) = state.duration {
                                state.position = duration;
                            }
                            let _ = events_tx.send(PlayerEvent::PlaybackEnded {
                                load_gen: state.load_gen,
                            });
                        }
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

    // ── §16 开放项 #2：中途读错误显式 Error 态（错误闩） ──────────────

    /// 最小合法 WAV（PCM 8-bit 单声道 8kHz）：44 字节头 + `data_len` 字节中值。
    fn wav_bytes(data_len: usize) -> Vec<u8> {
        let sample_rate = 8000u32;
        let mut b = Vec::with_capacity(44 + data_len);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // 单声道
        b.extend_from_slice(&sample_rate.to_le_bytes());
        b.extend_from_slice(&sample_rate.to_le_bytes()); // byte rate（8-bit mono）
        b.extend_from_slice(&1u16.to_le_bytes()); // block align
        b.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data_len as u32).to_le_bytes());
        b.extend_from_slice(&vec![128u8; data_len]);
        b
    }

    fn test_track(id: &str) -> hmp_core::Track {
        hmp_core::Track {
            id: hmp_core::TrackId::new(id),
            title: format!("t-{id}"),
            artists: vec![],
            album: None,
            duration: None,
            cover: None,
            url: None,
            available_qualities: vec![],
        }
    }

    /// 假进程内源：固定字节集；读越过 `err_at` 后返回 Err（模拟中途读
    /// 错误；`err_at = usize::MAX` 即永不失败的干净源）。
    #[derive(Debug)]
    struct FailingStreamSource {
        data: Vec<u8>,
        err_at: usize,
    }

    impl hmp_core::MediaStreamSource for FailingStreamSource {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }
        fn open(&self) -> std::io::Result<Box<dyn MediaStream>> {
            Ok(Box::new(FailingStream {
                data: self.data.clone(),
                err_at: self.err_at,
                pos: 0,
            }))
        }
    }

    struct FailingStream {
        data: Vec<u8>,
        err_at: usize,
        pos: usize,
    }

    impl std::io::Read for FailingStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.err_at {
                return Err(std::io::Error::other("synthetic mid-read failure"));
            }
            let end = self.data.len().min(self.err_at).min(self.pos + buf.len());
            let n = end.saturating_sub(self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..end]);
            self.pos = end;
            Ok(n)
        }
    }

    impl std::io::Seek for FailingStream {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            let base = match pos {
                std::io::SeekFrom::Start(n) => n as i128,
                std::io::SeekFrom::End(d) => self.data.len() as i128 + d as i128,
                std::io::SeekFrom::Current(d) => self.pos as i128 + d as i128,
            };
            self.pos = (base.clamp(0, self.data.len() as i128)) as usize;
            Ok(self.pos as u64)
        }
    }

    /// 收集驱动事件直到谓词命中或超时。
    async fn next_event(
        events: &mut tokio::sync::broadcast::Receiver<PlayerEvent>,
        deadline: tokio::time::Instant,
    ) -> Option<PlayerEvent> {
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, events.recv()).await {
                Ok(Ok(ev)) => return Some(ev),
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => return None,
                Err(_) => return None, // deadline 到
            }
        }
        None
    }

    /// §16：中途读错误不得被当成 EOS——显式 `PlayerEvent::Error` +
    /// `status=Error`，**无** PlaybackEnded（引擎侧因此不静默跳歌）。
    #[tokio::test]
    async fn mid_read_error_yields_error_event_not_ended() {
        let core = PlayerCore::new_silent_for_test();
        let mut events = core.subscribe_events();
        let state = core.subscribe_state();
        // 3s 素材，1s 处读错误（探测已过、解码中途命中）。
        let src = std::sync::Arc::new(FailingStreamSource {
            data: wav_bytes(24_000),
            err_at: 44 + 8_000,
        });
        core.load(LoadRequest {
            track: test_track("wav1"),
            uri: "fake://wav1".into(),
            quality: hmp_core::AudioQuality::Mp3_128,
            load_gen: 1,
            stream: Some(src),
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        let mut saw_error = false;
        let mut saw_ended = false;
        while let Some(ev) = next_event(&mut events, deadline).await {
            match ev {
                PlayerEvent::Error { .. } => {
                    saw_error = true;
                    break;
                }
                PlayerEvent::PlaybackEnded { .. } => {
                    saw_ended = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_error, "中途读错误应产生 Error 事件（而非静默终止）");
        assert!(!saw_ended, "中途读错误不得被当成曲尾 PlaybackEnded");
        // 状态翻到 Error（Error 事件先行，状态同拍随后；稍候收尾发布）。
        let settle = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < settle && state.borrow().status != PlaybackStatus::Error
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(state.borrow().status, PlaybackStatus::Error);
    }

    /// §16 防误伤：自然播完（无读错误）仍判 Ended + PlaybackEnded。
    #[tokio::test]
    async fn clean_exhaust_still_ends() {
        let core = PlayerCore::new_silent_for_test();
        let mut events = core.subscribe_events();
        let src = std::sync::Arc::new(FailingStreamSource {
            data: wav_bytes(2_400), // 0.3s
            err_at: usize::MAX,     // 永不失败
        });
        core.load(LoadRequest {
            track: test_track("wav2"),
            uri: "fake://wav2".into(),
            quality: hmp_core::AudioQuality::Mp3_128,
            load_gen: 1,
            stream: Some(src),
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        let mut saw_error = false;
        let mut saw_ended = false;
        while let Some(ev) = next_event(&mut events, deadline).await {
            match ev {
                PlayerEvent::Error { .. } => {
                    saw_error = true;
                    break;
                }
                PlayerEvent::PlaybackEnded { .. } => {
                    saw_ended = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_ended, "干净源自然播完应产生 PlaybackEnded");
        assert!(!saw_error, "干净源不得误报 Error");
    }

    /// O1：Error 态点 Play = 显式恢复——清闩后经 EOS 路径续播
    /// （PlaybackEnded 到达，不再闪回 Error）。
    #[tokio::test]
    async fn play_after_error_clears_latch_and_advances() {
        let core = PlayerCore::new_silent_for_test();
        let mut events = core.subscribe_events();
        let state = core.subscribe_state();
        let src = std::sync::Arc::new(FailingStreamSource {
            data: wav_bytes(24_000),
            err_at: 44 + 8_000,
        });
        core.load(LoadRequest {
            track: test_track("wav3"),
            uri: "fake://wav3".into(),
            quality: hmp_core::AudioQuality::Mp3_128,
            load_gen: 1,
            stream: Some(src),
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        // 先等到 Error。
        loop {
            let Some(ev) = next_event(&mut events, deadline).await else {
                panic!("8s 内未出现中途读错误 Error 事件");
            };
            if matches!(ev, PlayerEvent::Error { .. }) {
                break;
            }
        }
        assert_eq!(state.borrow().status, PlaybackStatus::Error);
        // 点播放：清闩 → 排空源判 Ended → PlaybackEnded（续播信号）。
        core.play();
        let mut saw_ended = false;
        let mut saw_second_error = false;
        while let Some(ev) = next_event(&mut events, deadline).await {
            match ev {
                PlayerEvent::PlaybackEnded { .. } => {
                    saw_ended = true;
                    break;
                }
                PlayerEvent::Error { .. } => {
                    saw_second_error = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(
            saw_ended && !saw_second_error,
            "Play 后应经 Ended 续播而非再次 Error（O1 清闩）"
        );
    }

    // ── §16 开放项 #3：seek 后台化（spawn_blocking + 双代际守卫 + ACK 超时）──

    /// condvar 门：`gate_open` 前所有 `seek()` 阻塞——模拟远端未缓冲区间
    /// seek 的探测读被 CDN 超时钳制（阻塞同时占用泵线程与 try_seek 调用者，
    /// 正是内联时代冻结 drive 循环的形态）。
    type SeekGate = std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>;

    fn new_gate() -> SeekGate {
        std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()))
    }

    fn gate_open(gate: &SeekGate) {
        let (lock, cvar) = &**gate;
        let mut open = lock.lock().unwrap();
        *open = true;
        cvar.notify_all();
    }

    /// 假进程内源：干净 WAV 数据 + 可门控阻塞的 seek。
    #[derive(Debug)]
    struct GatedSeekSource {
        data: Vec<u8>,
        gate: SeekGate,
    }

    impl hmp_core::MediaStreamSource for GatedSeekSource {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }
        fn open(&self) -> std::io::Result<Box<dyn MediaStream>> {
            Ok(Box::new(GatedSeekStream {
                data: self.data.clone(),
                pos: 0,
                gate: Arc::clone(&self.gate),
            }))
        }
    }

    struct GatedSeekStream {
        data: Vec<u8>,
        pos: usize,
        gate: SeekGate,
    }

    impl std::io::Read for GatedSeekStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let end = self.data.len().min(self.pos + buf.len());
            let n = end.saturating_sub(self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..end]);
            self.pos = end;
            Ok(n)
        }
    }

    impl std::io::Seek for GatedSeekStream {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            // 门未开：阻塞（等价于探测读挂在 CDN 超时上）。
            let (lock, cvar) = &*self.gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cvar.wait(open).unwrap();
            }
            let base = match pos {
                std::io::SeekFrom::Start(n) => n as i128,
                std::io::SeekFrom::End(d) => self.data.len() as i128 + d as i128,
                std::io::SeekFrom::Current(d) => self.pos as i128 + d as i128,
            };
            self.pos = (base.clamp(0, self.data.len() as i128)) as usize;
            Ok(self.pos as u64)
        }
    }

    /// 轮询等待状态谓词（watch 通道无 per-change 通知语义，轮询最简）。
    async fn wait_state(
        state: &watch::Receiver<PlaybackState>,
        within: Duration,
        pred: impl Fn(&PlaybackState) -> bool,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            if pred(&state.borrow()) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        pred(&state.borrow())
    }

    /// §16 #3 核心回归：seek 探测阻塞期间命令循环不得冻结——seek 在途
    /// （门关）时 Pause 必须即时生效（红：旧内联 try_seek 同步等音频线程，
    /// Pause 排队直至 seek 完成；绿：驱动即时翻转）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn seek_in_flight_does_not_block_commands() {
        let core = PlayerCore::new_silent_for_test();
        let state = core.subscribe_state();
        let gate = new_gate();
        let src = std::sync::Arc::new(GatedSeekSource {
            data: wav_bytes(240_000), // 30s
            gate: Arc::clone(&gate),
        });
        core.load(LoadRequest {
            track: test_track("g1"),
            uri: "fake://g1".into(),
            quality: hmp_core::AudioQuality::Mp3_128,
            load_gen: 1,
            stream: Some(src),
        });
        assert!(
            wait_state(&state, Duration::from_secs(5), |s| s.status
                == PlaybackStatus::Playing)
            .await,
            "装载后应起播"
        );

        core.seek(Duration::from_secs(10));
        // 留一个调度节拍让 seek 进入门控（泵线程卡在 attempt）。
        tokio::time::sleep(Duration::from_millis(100)).await;
        core.pause();
        assert!(
            wait_state(&state, Duration::from_secs(1), |s| s.status
                == PlaybackStatus::Paused)
            .await,
            "seek 在途时 Pause 必须即时生效（命令循环不得冻结）"
        );

        gate_open(&gate);
        // 放行后 seek 结果按守卫应用：保持 Paused（用户意图优先），
        // 位置到达目标 10s（未被门控期间的自然播放推进覆盖）。
        assert!(
            wait_state(&state, Duration::from_secs(5), |s| s.status
                == PlaybackStatus::Paused
                && s.position >= Duration::from_secs(9)
                && s.position <= Duration::from_secs(11))
            .await,
            "放行后 seek 结果应生效（位置 ≈10s，保持 Paused），实际 {:?}",
            state.borrow().position
        );
    }

    /// §16 #3：seek 在途时新装载 ACK → load_gen 守卫丢弃迟到 seek 结果，
    /// 新曲位置不被旧 seek 污染。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn new_load_invalidates_pending_seek_outcome() {
        let core = PlayerCore::new_silent_for_test();
        let state = core.subscribe_state();
        let gate = new_gate();
        let src1 = std::sync::Arc::new(GatedSeekSource {
            data: wav_bytes(240_000),
            gate: Arc::clone(&gate),
        });
        core.load(LoadRequest {
            track: test_track("h1"),
            uri: "fake://h1".into(),
            quality: hmp_core::AudioQuality::Mp3_128,
            load_gen: 1,
            stream: Some(src1),
        });
        assert!(
            wait_state(&state, Duration::from_secs(5), |s| s.status
                == PlaybackStatus::Playing)
            .await
        );

        core.seek(Duration::from_secs(25));
        tokio::time::sleep(Duration::from_millis(100)).await; // seek 卡进门控
        let src2 = std::sync::Arc::new(FailingStreamSource {
            data: wav_bytes(240_000),
            err_at: usize::MAX,
        });
        core.load(LoadRequest {
            track: test_track("h2"),
            uri: "fake://h2".into(),
            quality: hmp_core::AudioQuality::Mp3_128,
            load_gen: 2,
            stream: Some(src2),
        });
        tokio::time::sleep(Duration::from_millis(100)).await; // load 已分派
        gate_open(&gate); // 放行 → 旧 seek 完成、旧源被清、新装载完成
        assert!(
            wait_state(&state, Duration::from_secs(5), |s| s.status
                == PlaybackStatus::Playing
                && s.current.as_ref().map(|t| t.id.as_ref()) == Some("h2"))
            .await,
            "放行后新装载应完成起播"
        );
        // 迟到的 seek(25s) 结果若漏防会立即把位置顶到 25s。
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            state.borrow().position < Duration::from_secs(5),
            "新曲位置不得被旧 seek 结果覆盖（目标 25s），实际 {:?}",
            state.borrow().position
        );
    }

    /// §16 #3：后发 seek 取代前者——终态位置 = 后发者（completion 序 ≡
    /// 派发序，前者的迟到结果按 seek_gen 守卫丢弃）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn second_seek_supersedes_first() {
        let core = PlayerCore::new_silent_for_test();
        let state = core.subscribe_state();
        let gate = new_gate();
        let src = std::sync::Arc::new(GatedSeekSource {
            data: wav_bytes(240_000),
            gate: Arc::clone(&gate),
        });
        core.load(LoadRequest {
            track: test_track("s1"),
            uri: "fake://s1".into(),
            quality: hmp_core::AudioQuality::Mp3_128,
            load_gen: 1,
            stream: Some(src),
        });
        assert!(
            wait_state(&state, Duration::from_secs(5), |s| s.status
                == PlaybackStatus::Playing)
            .await
        );

        core.seek(Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(100)).await; // seek1 卡住
        core.seek(Duration::from_secs(20));
        tokio::time::sleep(Duration::from_millis(100)).await; // seek2 覆盖槽位
        gate_open(&gate);
        assert!(
            wait_state(&state, Duration::from_secs(5), |s| s.position
                >= Duration::from_secs(18)
                && s.position <= Duration::from_secs(23))
            .await,
            "终态位置应为后发 seek（≈20s），实际 {:?}",
            state.borrow().position
        );
    }

    /// M3（§16 #3）：Stop 失效在途 seek——迟到结果不污染 Stopped 态位置。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_invalidates_pending_seek_outcome() {
        let core = PlayerCore::new_silent_for_test();
        let state = core.subscribe_state();
        let gate = new_gate();
        let src = std::sync::Arc::new(GatedSeekSource {
            data: wav_bytes(240_000),
            gate: Arc::clone(&gate),
        });
        core.load(LoadRequest {
            track: test_track("m1"),
            uri: "fake://m1".into(),
            quality: hmp_core::AudioQuality::Mp3_128,
            load_gen: 1,
            stream: Some(src),
        });
        assert!(
            wait_state(&state, Duration::from_secs(5), |s| s.status
                == PlaybackStatus::Playing)
            .await
        );

        core.seek(Duration::from_secs(25));
        tokio::time::sleep(Duration::from_millis(100)).await; // seek 卡进门控
        core.stop();
        assert!(
            wait_state(&state, Duration::from_secs(1), |s| s.status
                == PlaybackStatus::Stopped
                && s.position == Duration::ZERO)
            .await,
            "Stop 应即时生效且位置归零"
        );
        gate_open(&gate); // 迟到 seek 结果到达 → 按 seek_gen 守卫丢弃
        tokio::time::sleep(Duration::from_millis(300)).await;
        let s = state.borrow();
        assert_eq!(s.status, PlaybackStatus::Stopped, "状态保持 Stopped");
        assert_eq!(
            s.position,
            Duration::ZERO,
            "迟到 seek 结果不得污染 Stopped 态位置"
        );
    }
}
