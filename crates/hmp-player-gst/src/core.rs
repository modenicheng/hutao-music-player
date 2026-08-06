//! GStreamer 播放器核心实现。

use std::time::Duration;

use gstreamer::glib::prelude::*;
use gstreamer_player::{
    Player, PlayerGMainContextSignalDispatcher, PlayerState, PlayerVideoOverlayVideoRenderer,
};
use hmp_core::{HmpError, LoopMode, PlaybackState, PlaybackStatus, PlayerCommand, Track};
use tokio::sync::{broadcast, mpsc, watch};

use crate::events::PlayerEvent;

/// 加载请求：曲目元数据 + 播放 URI。
#[derive(Clone, Debug)]
pub struct LoadRequest {
    /// 曲目元数据（供状态发布与上层展示）。
    pub track: Track,
    /// 播放 URI（http/https/本地文件）。
    pub uri: String,
    /// 请求音质（记录用）。
    pub quality: hmp_core::AudioQuality,
}

/// 加载请求（与命令分离：`Load` 携带 URI/元数据，命令经公共通道）。
enum LoadCommand {
    Load(Box<LoadRequest>),
    Shutdown,
}

/// GStreamer 播放器核心。
///
/// 单一播放状态源：状态经 `watch` 发布、离散事件经 `broadcast` 发布；
/// 调用方（UI/MPRIS/CLI）通过方法发送命令，禁止自行推算进度。
pub struct PlayerCore {
    cmd_tx: mpsc::UnboundedSender<PlayerCommand>,
    load_tx: mpsc::UnboundedSender<LoadCommand>,
    state_rx: watch::Receiver<PlaybackState>,
    events_rx: broadcast::Receiver<PlayerEvent>,
}

impl PlayerCore {
    /// 初始化 GStreamer 并启动播放器核心。
    pub fn new() -> Result<Self, HmpError> {
        Self::new_with_sink(None)
    }

    /// 初始化播放器并指定音频 sink 元素名。
    ///
    /// `None` 使用自动探测（autoaudiosink）；无音频设备的环境（CI/容器）
    /// 可传 `Some("fakeaudiosink")` 以便验证状态机。
    pub fn new_with_sink(audio_sink: Option<&str>) -> Result<Self, HmpError> {
        gstreamer::init().map_err(|e| HmpError::Playback(format!("gstreamer init: {e}")))?;
        // 默认播放器：视频输出用 overlay 渲染器（音频为主，视频备用）；
        // 信号经主上下文调度器派发
        let player = Player::new(
            None::<PlayerVideoOverlayVideoRenderer>,
            None::<PlayerGMainContextSignalDispatcher>,
        );
        if let Some(sink_name) = audio_sink {
            let sink = gstreamer::ElementFactory::make(sink_name)
                .build()
                .map_err(|e| HmpError::Playback(format!("create audio sink {sink_name}: {e}")))?;
            player.pipeline().set_property("audio-sink", &sink);
        }
        Self::from_player(player)
    }

    /// 用现成的 `Player` 构造核心（测试/定制注入用）。
    pub fn from_player(player: Player) -> Result<Self, HmpError> {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (load_tx, load_rx) = mpsc::unbounded_channel();
        let (state_tx, state_rx) = watch::channel(PlaybackState::default());
        let (events_tx, events_rx) = broadcast::channel(64);

        // GStreamer 回调 → 异步驱动循环的事件转发
        let (bus_tx, bus_rx) = mpsc::unbounded_channel::<BusEvent>();

        let bus_tx2 = bus_tx.clone();
        player.connect_state_changed(move |p, state| {
            let _ = bus_tx2.send(BusEvent::StateChanged(state));
            // duration-notify 信号在部分 sink 下不触发，状态变化时主动查询
            let _ = bus_tx2.send(BusEvent::Duration(p.duration()));
        });
        let bus_tx2 = bus_tx.clone();
        player.connect_error(move |_, error| {
            let _ = bus_tx2.send(BusEvent::Error(error.to_string()));
        });
        let bus_tx2 = bus_tx.clone();
        player.connect_end_of_stream(move |_| {
            let _ = bus_tx2.send(BusEvent::Eos);
        });
        let bus_tx2 = bus_tx.clone();
        player.connect_position_updated(move |_, position| {
            let _ = bus_tx2.send(BusEvent::Position(position));
        });
        let bus_tx2 = bus_tx.clone();
        player.connect_duration_notify(move |p| {
            let _ = bus_tx2.send(BusEvent::Duration(p.duration()));
        });
        let bus_tx2 = bus_tx.clone();
        player.connect_buffering(move |_, percent| {
            let _ = bus_tx2.send(BusEvent::Buffering(percent));
        });

        let core = PlayerCore {
            cmd_tx,
            load_tx,
            state_rx,
            events_rx,
        };
        tokio::spawn(drive(player, cmd_rx, load_rx, state_tx, events_tx, bus_rx));
        Ok(core)
    }

    /// 加载并播放 URI（进入 Loading → Playing）。
    pub fn load(&self, request: LoadRequest) {
        let _ = self.load_tx.send(LoadCommand::Load(Box::new(request)));
    }

    /// 播放。
    pub fn play(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Play);
    }

    /// 暂停。
    pub fn pause(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Pause);
    }

    /// 停止。
    pub fn stop(&self) {
        let _ = self.cmd_tx.send(PlayerCommand::Stop);
    }

    /// 跳转。
    pub fn seek(&self, position: Duration) {
        let _ = self.cmd_tx.send(PlayerCommand::Seek(position));
    }

    /// 设置音量（0.0..=1.0）。
    pub fn set_volume(&self, volume: f64) {
        let _ = self.cmd_tx.send(PlayerCommand::SetVolume(volume));
    }

    /// 设置循环模式（记录于状态）。
    pub fn set_loop_mode(&self, mode: LoopMode) {
        let _ = self.cmd_tx.send(PlayerCommand::SetLoopMode(mode));
    }

    /// 停止驱动循环并释放播放器。
    pub fn shutdown(&self) {
        let _ = self.load_tx.send(LoadCommand::Shutdown);
    }

    /// 播放器命令发送端（供 MPRIS/上层命令转发）。
    pub fn command_sender(&self) -> mpsc::UnboundedSender<PlayerCommand> {
        self.cmd_tx.clone()
    }

    /// 订阅播放状态（watch 通道）。
    pub fn subscribe_state(&self) -> watch::Receiver<PlaybackState> {
        self.state_rx.clone()
    }

    /// 订阅离散事件（broadcast 通道）。
    pub fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent> {
        self.events_rx.resubscribe()
    }
}

/// GStreamer 回调转发的事件。
enum BusEvent {
    StateChanged(PlayerState),
    Error(String),
    Eos,
    Position(Option<gstreamer::ClockTime>),
    Duration(Option<gstreamer::ClockTime>),
    Buffering(i32),
}

/// 驱动循环：消费命令 + 总线事件，维护状态机并发布。
#[allow(clippy::too_many_arguments)]
async fn drive(
    player: Player,
    mut cmd_rx: mpsc::UnboundedReceiver<PlayerCommand>,
    mut load_rx: mpsc::UnboundedReceiver<LoadCommand>,
    state_tx: watch::Sender<PlaybackState>,
    events_tx: broadcast::Sender<PlayerEvent>,
    mut bus_rx: mpsc::UnboundedReceiver<BusEvent>,
) {
    let mut state = PlaybackState::default();
    let mut pending_error: Option<String> = None;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { continue };
                match cmd {
                    PlayerCommand::Play => {
                        player.play();
                        state.status = PlaybackStatus::Playing;
                        let _ = state_tx.send(state.clone());
                    }
                    PlayerCommand::Pause => {
                        player.pause();
                        state.status = PlaybackStatus::Paused;
                        let _ = state_tx.send(state.clone());
                    }
                    PlayerCommand::Stop => {
                        player.stop();
                        state.status = PlaybackStatus::Stopped;
                        state.position = Duration::ZERO;
                        let _ = state_tx.send(state.clone());
                    }
                    PlayerCommand::Seek(pos) => {
                        player.seek(gstreamer::ClockTime::from_nseconds(
                            pos.as_nanos().min(u64::MAX as u128) as u64,
                        ));
                        state.position = pos;
                        let _ = state_tx.send(state.clone());
                    }
                    PlayerCommand::SetVolume(vol) => {
                        player.set_volume(vol.clamp(0.0, 1.0));
                        state.volume = vol.clamp(0.0, 1.0);
                        let _ = state_tx.send(state.clone());
                    }
                    PlayerCommand::SetLoopMode(mode) => {
                        state.loop_mode = mode;
                        let _ = state_tx.send(state.clone());
                    }
                    // Next/Previous/SetShuffle/LoadAndPlay：由上层应用核心消费，
                    // 播放器核心不直接处理（无队列语义）
                    PlayerCommand::Next
                    | PlayerCommand::Previous
                    | PlayerCommand::SetShuffle(_)
                    | PlayerCommand::LoadAndPlay(_) => {}
                }
            }
            load = load_rx.recv() => {
                let Some(load_cmd) = load else { continue };
                match load_cmd {
                    LoadCommand::Shutdown => break,
                    LoadCommand::Load(req) => {
                        // 加载即播放（docs/PROJECT.md §8.2：设置 URI → Loading → Playing）
                        state.status = PlaybackStatus::Loading;
                        state.current = Some(req.track);
                        state.position = Duration::ZERO;
                        state.duration = None;
                        state.buffering = None;
                        player.set_uri(Some(req.uri.as_str()));
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::TrackChanged);
                        player.play();
                        state.status = PlaybackStatus::Playing;
                        let _ = state_tx.send(state.clone());
                    }
                }
            }
            bus = bus_rx.recv() => {
                let Some(evt) = bus else { continue };
                match evt {
                    BusEvent::StateChanged(ps) => {
                        // PlayerState 仅含 Stopped/Buffering/Paused/Playing；
                        // Loading/Empty/Ended 由本核心依据加载动作显式维护。
                        let status = match ps {
                            PlayerState::Stopped => {
                                if state.status == PlaybackStatus::Error {
                                    state.status
                                } else if state.current.is_some() {
                                    PlaybackStatus::Stopped
                                } else {
                                    PlaybackStatus::Empty
                                }
                            }
                            PlayerState::Playing => PlaybackStatus::Playing,
                            PlayerState::Paused => PlaybackStatus::Paused,
                            PlayerState::Buffering => PlaybackStatus::Buffering,
                            _ => state.status,
                        };
                        state.status = status;
                        let _ = state_tx.send(state.clone());
                    }
                    BusEvent::Error(msg) => {
                        pending_error = Some(msg.clone());
                        state.status = PlaybackStatus::Error;
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::Error(
                            HmpError::Playback(msg),
                        ));
                    }
                    BusEvent::Eos => {
                        state.status = PlaybackStatus::Ended;
                        state.position = state.duration.unwrap_or(state.position);
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::PlaybackEnded);
                    }
                    BusEvent::Position(pos) => {
                        if let Some(ct) = pos {
                            state.position = Duration::from_nanos(ct.nseconds());
                            let _ = state_tx.send(state.clone());
                        }
                    }
                    BusEvent::Duration(dur) => {
                        if let Some(ct) = dur {
                            state.duration = Some(Duration::from_nanos(ct.nseconds()));
                            state.can_seek = true;
                            let _ = state_tx.send(state.clone());
                        }
                    }
                    BusEvent::Buffering(percent) => {
                        state.buffering = if percent < 100 {
                            Some(percent as f64 / 100.0)
                        } else {
                            None
                        };
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::BufferingChanged(state.buffering));
                    }
                }
                let _ = pending_error.take();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmp_core::{AudioQuality, TrackId};

    fn sample_track() -> Track {
        Track::new(TrackId::new("test-1"), "测试曲目")
    }

    fn load_req(track: Track, uri: &str) -> LoadRequest {
        LoadRequest {
            track,
            uri: uri.to_owned(),
            quality: AudioQuality::Mp3_128,
        }
    }

    #[tokio::test]
    async fn default_state_is_empty() {
        let core = PlayerCore::new_with_sink(Some("fakeaudiosink")).expect("init");
        let state = core.subscribe_state().borrow().clone();
        assert_eq!(state.status, PlaybackStatus::Empty);
        assert!(state.current.is_none());
        core.shutdown();
    }

    #[tokio::test]
    async fn load_publishes_track_and_loading() {
        let core = PlayerCore::new_with_sink(Some("fakeaudiosink")).expect("init");
        let rx = core.subscribe_state();
        let track = sample_track();
        core.load(load_req(track.clone(), "file:///nonexistent.aiff"));
        // 等待状态发布（驱动循环异步处理）
        tokio::time::sleep(Duration::from_millis(100)).await;
        let s = rx.borrow().clone();
        assert_eq!(s.current.as_ref().unwrap().id, TrackId::new("test-1"));
        assert!(matches!(
            s.status,
            PlaybackStatus::Loading | PlaybackStatus::Error
        ));
        core.shutdown();
    }

    #[tokio::test]
    async fn set_volume_is_published() {
        let core = PlayerCore::new_with_sink(Some("fakeaudiosink")).expect("init");
        let rx = core.subscribe_state();
        core.set_volume(0.33);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(rx.borrow().volume, 0.33);
        core.shutdown();
    }

    #[tokio::test]
    async fn loop_mode_is_published() {
        let core = PlayerCore::new_with_sink(Some("fakeaudiosink")).expect("init");
        let rx = core.subscribe_state();
        core.set_loop_mode(LoopMode::Track);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(rx.borrow().loop_mode, LoopMode::Track);
        core.shutdown();
    }

    #[tokio::test]
    async fn error_event_on_bad_uri() {
        let core = PlayerCore::new_with_sink(Some("fakeaudiosink")).expect("init");
        let rx = core.subscribe_state();
        let mut ev = core.subscribe_events();
        core.load(load_req(
            sample_track(),
            "file:///definitely/missing/file.aiff",
        ));
        core.play();
        let mut saw_error = false;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Ok(e) = ev.try_recv() {
                if matches!(e, PlayerEvent::Error(_)) {
                    saw_error = true;
                }
            }
            if rx.borrow().status == PlaybackStatus::Error {
                saw_error = true;
            }
            if saw_error {
                break;
            }
        }
        assert!(saw_error, "missing file should produce error");
        core.shutdown();
    }
}
