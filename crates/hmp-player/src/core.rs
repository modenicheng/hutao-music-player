//! Rodio-backed player state machine.

use std::fs::File;
use std::sync::Arc;
use std::time::Duration;

use hmp_core::{HmpError, LoadRequest, PlaybackState, PlaybackStatus, PlayerCommand, PlayerEvent};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};
use stream_download::storage::temp::TempStorageProvider;
use stream_download::{Settings, StreamDownload};
use tokio::sync::{broadcast, mpsc, watch};

use crate::source::{MediaLocation, parse_uri};

enum LoadCommand {
    Load(Box<LoadRequest>),
    Shutdown,
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
        let output = OutputStreamBuilder::open_default_stream()
            .map_err(|error| HmpError::Playback(format!("open default audio output: {error}")))?;
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
