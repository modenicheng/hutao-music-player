//! Rodio-backed player state machine.

use std::fs::File;
use std::sync::Arc;
use std::time::Duration;

use hmp_core::{HmpError, LoadRequest, PlaybackState, PlaybackStatus, PlayerCommand, PlayerEvent};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source, StreamError};
use stream_download::storage::temp::TempStorageProvider;
use stream_download::{Settings, StreamDownload};
use tokio::sync::{broadcast, mpsc, watch};

use crate::source::{MediaLocation, parse_uri};

enum LoadCommand {
    Load(Box<LoadRequest>),
    Shutdown,
}

/// Open a server-routed output stream (safe variant).
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
fn open_default_output() -> Result<OutputStream, StreamError> {
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
    candidates.sort_by_key(|&(preference, _)| preference);

    let mut last_error = StreamError::NoDevice;
    for (_, device) in candidates {
        let name = device.name().unwrap_or_else(|_| "<unnamed>".into());
        let builder = match OutputStreamBuilder::from_device(device) {
            Ok(builder) => builder,
            Err(error) => {
                tracing::warn!(%name, %error, "server-routed pcm failed, trying next");
                last_error = error;
                continue;
            }
        };
        match builder.open_stream() {
            Ok(stream) => {
                tracing::info!(%name, "audio output opened via server-routed pcm");
                return Ok(stream);
            }
            Err(error) => {
                tracing::warn!(%name, %error, "server-routed pcm failed, trying next");
                last_error = error;
            }
        }
    }
    Err(last_error)
}

/// Server-routed PCM policy: preference order of PCM names that always route
/// through the system audio server. Direct hardware PCMs (`hw:*`, `plughw:*`,
/// `front:*`, `surround*:*`), OSS (`oss`) and JACK (`jack`) are refused so the
/// player can never grab exclusive hardware behind PipeWire/PulseAudio.
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
    pub fn new() -> Result<Self, HmpError> {
        let output = open_default_output().map_err(|error| {
            HmpError::Playback(format!(
                "open default audio output: {error} (no system audio server reachable?)"
            ))
        })?;
        let sink = Sink::connect_new(output.mixer());
        Ok(Self::from_sink(sink, Some(output)))
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
    match location {
        MediaLocation::File(path) => {
            let file = File::open(&path).map_err(|error| {
                HmpError::Playback(format!("open audio file {}: {error}", path.display()))
            })?;
            let decoder = Decoder::try_from(file).map_err(|error| {
                HmpError::Playback(format!("decode audio file {}: {error}", path.display()))
            })?;
            let duration = decoder.total_duration();
            sink.append(decoder);
            Ok(duration)
        }
        MediaLocation::Http(uri) => {
            let stream =
                StreamDownload::new_http(uri, TempStorageProvider::new(), Settings::default())
                    .await
                    .map_err(|error| HmpError::Playback(format!("open audio stream: {error}")))?;
            let decoder = Decoder::builder()
                .with_data(stream)
                .build()
                .map_err(|error| HmpError::Playback(format!("decode audio stream: {error}")))?;
            let duration = decoder.total_duration();
            sink.append(decoder);
            Ok(duration)
        }
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
                        sink.stop();
                        state.status = PlaybackStatus::Stopped;
                        state.position = Duration::ZERO;
                    }
                    PlayerCommand::Seek(position) => {
                        match sink.try_seek(position) {
                            Ok(()) => state.position = position,
                            Err(error) => {
                                let error = HmpError::Playback(format!("seek audio stream: {error}"));
                                state.status = PlaybackStatus::Error;
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
                    LoadCommand::Shutdown => break,
                    LoadCommand::Load(request) => {
                        state.status = PlaybackStatus::Loading;
                        state.buffering = Some(0.0);
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::BufferingChanged(Some(0.0)));

                        let result = match parse_uri(&request.uri) {
                            Ok(location) => {
                                sink.clear();
                                append_location(&sink, location).await
                            }
                            Err(error) => Err(error),
                        };
                        match result {
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
    use super::*;

    /// 输出设备安全策略：只允许经系统音频服务器路由的 PCM。回归守护：
    /// 防止再次引入"枚举/直通设备 fallback"而独占硬件
    /// （2026-09-07 DAWN PRO2 被抢事件）。
    #[test]
    fn output_policy_accepts_only_server_routed_pcms() {
        assert_eq!(server_routed_preference("default"), Some(0));
        assert_eq!(server_routed_preference("pipewire"), Some(1));
        assert_eq!(server_routed_preference("pulse"), Some(2));
    }

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
