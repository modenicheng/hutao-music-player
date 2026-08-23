//! Windows Runtime owner for System Media Transport Controls.

use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::Duration,
};

use hmp_core::{LoopMode, PlaybackCapabilities, PlaybackState, PlayerCommand};
use tokio::sync::{mpsc as tokio_mpsc, watch};
use windows::{
    Foundation::{TimeSpan, TypedEventHandler, Uri},
    Media::Playback::MediaPlayer,
    Media::{
        AutoRepeatModeChangeRequestedEventArgs, MediaPlaybackAutoRepeatMode, MediaPlaybackStatus,
        MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs,
        ShuffleEnabledChangeRequestedEventArgs, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
        SystemMediaTransportControlsTimelineProperties,
    },
    Storage::Streams::RandomAccessStreamReference,
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::{HSTRING, Result as WindowsResult},
};

use crate::model::{ProjectedButton, ProjectedStatus, Projection, map_button, map_repeat_request};

#[derive(Debug, thiserror::Error)]
pub enum SmtcError {
    #[error("failed to initialize Windows media controls: {0}")]
    Initialization(String),
}

enum ControlMessage {
    Update(Projection),
    Stop,
}

/// Owns the Windows media session and the state-forwarding task.
///
/// Dropping the guard unregisters all WinRT callbacks and closes the helper
/// `MediaPlayer`. The helper never receives an audio source; it exists only to
/// expose `SystemMediaTransportControls` for the Rodio-backed daemon.
pub struct SmtcService {
    control_tx: Sender<ControlMessage>,
    forward_task: tokio::task::JoinHandle<()>,
    owner_thread: Option<JoinHandle<()>>,
}

impl SmtcService {
    pub fn start(
        command_tx: tokio_mpsc::UnboundedSender<PlayerCommand>,
        mut state_rx: watch::Receiver<PlaybackState>,
        mut capabilities_rx: watch::Receiver<PlaybackCapabilities>,
    ) -> Result<Self, SmtcError> {
        let (control_tx, control_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let owner_thread = thread::Builder::new()
            .name("hmp-smtc".into())
            .spawn(move || run_owner(control_rx, ready_tx, command_tx))
            .map_err(|error| SmtcError::Initialization(error.to_string()))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let _ = owner_thread.join();
                return Err(SmtcError::Initialization(error));
            }
            Err(error) => {
                let _ = owner_thread.join();
                return Err(SmtcError::Initialization(error.to_string()));
            }
        }

        let forward_tx = control_tx.clone();
        let forward_task = tokio::spawn(async move {
            loop {
                let projection = Projection::from_state(
                    &state_rx.borrow_and_update(),
                    *capabilities_rx.borrow_and_update(),
                );
                if forward_tx.send(ControlMessage::Update(projection)).is_err() {
                    break;
                }

                tokio::select! {
                    changed = state_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                    changed = capabilities_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            control_tx,
            forward_task,
            owner_thread: Some(owner_thread),
        })
    }
}

impl Drop for SmtcService {
    fn drop(&mut self) {
        self.forward_task.abort();
        let _ = self.control_tx.send(ControlMessage::Stop);
        if let Some(owner_thread) = self.owner_thread.take() {
            let _ = owner_thread.join();
        }
    }
}

fn run_owner(
    control_rx: Receiver<ControlMessage>,
    ready_tx: mpsc::SyncSender<Result<(), String>>,
    command_tx: tokio_mpsc::UnboundedSender<PlayerCommand>,
) {
    // SAFETY: this thread owns a balanced WinRT MTA initialization for its
    // complete lifetime, and all WinRT objects are dropped before uninitialize.
    let apartment = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    if let Err(error) = apartment {
        let _ = ready_tx.send(Err(error.to_string()));
        return;
    }

    let owner = SmtcOwner::new(command_tx);
    match owner {
        Ok(owner) => {
            let _ = ready_tx.send(Ok(()));
            owner.run(control_rx);
        }
        Err(error) => {
            let _ = ready_tx.send(Err(error.to_string()));
        }
    }

    // SAFETY: paired with the successful `RoInitialize` above, on the same
    // thread, after every WinRT object has been dropped.
    unsafe { RoUninitialize() };
}

struct SmtcOwner {
    player: MediaPlayer,
    controls: SystemMediaTransportControls,
    button_token: i64,
    position_token: i64,
    shuffle_token: i64,
    repeat_token: i64,
}

impl SmtcOwner {
    fn new(command_tx: tokio_mpsc::UnboundedSender<PlayerCommand>) -> WindowsResult<Self> {
        let player = MediaPlayer::new()?;
        player.CommandManager()?.SetIsEnabled(false)?;
        let controls = player.SystemMediaTransportControls()?;

        let button_tx = command_tx.clone();
        let button_handler: TypedEventHandler<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        > = TypedEventHandler::new(
            move |_sender,
                  args: windows::core::Ref<
                '_,
                SystemMediaTransportControlsButtonPressedEventArgs,
            >| {
                let button = args.ok()?.Button()?;
                if let Some(command) = project_button(button).and_then(map_button) {
                    let _ = button_tx.send(command);
                }
                Ok(())
            },
        );
        let button_token = controls.ButtonPressed(&button_handler)?;

        let position_tx = command_tx.clone();
        let position_handler: TypedEventHandler<
            SystemMediaTransportControls,
            PlaybackPositionChangeRequestedEventArgs,
        > = TypedEventHandler::new(move |_sender, args: windows::core::Ref<'_, PlaybackPositionChangeRequestedEventArgs>| {
            let position = duration_from_time_span(args.ok()?.RequestedPlaybackPosition()?);
            let _ = position_tx.send(PlayerCommand::Seek(position));
            Ok(())
        });
        let position_token = controls.PlaybackPositionChangeRequested(&position_handler)?;

        let shuffle_tx = command_tx.clone();
        let shuffle_handler: TypedEventHandler<
            SystemMediaTransportControls,
            ShuffleEnabledChangeRequestedEventArgs,
        > = TypedEventHandler::new(
            move |_sender, args: windows::core::Ref<'_, ShuffleEnabledChangeRequestedEventArgs>| {
                let enabled = args.ok()?.RequestedShuffleEnabled()?;
                let _ = shuffle_tx.send(PlayerCommand::SetShuffle(enabled));
                Ok(())
            },
        );
        let shuffle_token = controls.ShuffleEnabledChangeRequested(&shuffle_handler)?;

        let repeat_handler: TypedEventHandler<
            SystemMediaTransportControls,
            AutoRepeatModeChangeRequestedEventArgs,
        > = TypedEventHandler::new(
            move |_sender, args: windows::core::Ref<'_, AutoRepeatModeChangeRequestedEventArgs>| {
                let requested = args.ok()?.RequestedAutoRepeatMode()?;
                if let Some(command) = map_repeat_request(requested.0) {
                    let _ = command_tx.send(command);
                }
                Ok(())
            },
        );
        let repeat_token = controls.AutoRepeatModeChangeRequested(&repeat_handler)?;

        controls.SetIsEnabled(true)?;
        Ok(Self {
            player,
            controls,
            button_token,
            position_token,
            shuffle_token,
            repeat_token,
        })
    }

    fn run(self, control_rx: Receiver<ControlMessage>) {
        while let Ok(message) = control_rx.recv() {
            match message {
                ControlMessage::Update(projection) => {
                    if let Err(error) = self.apply_projection(&projection) {
                        tracing::warn!(%error, "failed to update Windows media controls");
                    }
                }
                ControlMessage::Stop => break,
            }
        }
    }

    fn apply_projection(&self, projection: &Projection) -> WindowsResult<()> {
        self.controls.SetIsPlayEnabled(projection.can_play)?;
        self.controls.SetIsPauseEnabled(projection.can_pause)?;
        self.controls.SetIsStopEnabled(projection.can_stop)?;
        self.controls.SetIsNextEnabled(projection.can_next)?;
        self.controls
            .SetIsPreviousEnabled(projection.can_previous)?;
        self.controls
            .SetPlaybackStatus(project_status(projection.status))?;
        self.controls.SetShuffleEnabled(projection.shuffle)?;
        self.controls
            .SetAutoRepeatMode(project_loop_mode(projection.loop_mode))?;

        self.update_metadata(projection)?;
        self.update_timeline(projection)?;
        Ok(())
    }

    fn update_metadata(&self, projection: &Projection) -> WindowsResult<()> {
        let updater = self.controls.DisplayUpdater()?;
        updater.ClearAll()?;
        if let Some(title) = &projection.title {
            updater.SetType(MediaPlaybackType::Music)?;
            let music = updater.MusicProperties()?;
            music.SetTitle(&HSTRING::from(title))?;
            music.SetArtist(&HSTRING::from(
                projection.artist.as_deref().unwrap_or_default(),
            ))?;
            music.SetAlbumTitle(&HSTRING::from(
                projection.album.as_deref().unwrap_or_default(),
            ))?;

            if let Some(cover_url) = &projection.cover_url {
                match Uri::CreateUri(&HSTRING::from(cover_url))
                    .and_then(|uri| RandomAccessStreamReference::CreateFromUri(&uri))
                {
                    Ok(thumbnail) => updater.SetThumbnail(&thumbnail)?,
                    Err(error) => {
                        tracing::debug!(%error, %cover_url, "ignored unsupported SMTC cover URI");
                    }
                }
            }
        }
        updater.Update()
    }

    fn update_timeline(&self, projection: &Projection) -> WindowsResult<()> {
        let zero = TimeSpan { Duration: 0 };
        let duration = projection.duration.unwrap_or_default();
        let end = time_span(duration);
        let position = time_span(projection.position.min(duration));
        let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
        timeline.SetStartTime(zero)?;
        timeline.SetEndTime(end)?;
        timeline.SetPosition(position)?;
        timeline.SetMinSeekTime(zero)?;
        timeline.SetMaxSeekTime(if projection.can_seek { end } else { zero })?;
        self.controls.UpdateTimelineProperties(&timeline)
    }
}

impl Drop for SmtcOwner {
    fn drop(&mut self) {
        let _ = self.controls.RemoveButtonPressed(self.button_token);
        let _ = self
            .controls
            .RemovePlaybackPositionChangeRequested(self.position_token);
        let _ = self
            .controls
            .RemoveShuffleEnabledChangeRequested(self.shuffle_token);
        let _ = self
            .controls
            .RemoveAutoRepeatModeChangeRequested(self.repeat_token);
        let _ = self.controls.SetIsEnabled(false);
        let _ = self.player.Close();
    }
}

fn project_button(button: SystemMediaTransportControlsButton) -> Option<ProjectedButton> {
    match button {
        SystemMediaTransportControlsButton::Play => Some(ProjectedButton::Play),
        SystemMediaTransportControlsButton::Pause => Some(ProjectedButton::Pause),
        SystemMediaTransportControlsButton::Stop => Some(ProjectedButton::Stop),
        SystemMediaTransportControlsButton::Next => Some(ProjectedButton::Next),
        SystemMediaTransportControlsButton::Previous => Some(ProjectedButton::Previous),
        _ => None,
    }
}

const fn project_status(status: ProjectedStatus) -> MediaPlaybackStatus {
    match status {
        ProjectedStatus::Playing => MediaPlaybackStatus::Playing,
        ProjectedStatus::Paused => MediaPlaybackStatus::Paused,
        ProjectedStatus::Stopped => MediaPlaybackStatus::Stopped,
    }
}

const fn project_loop_mode(mode: LoopMode) -> MediaPlaybackAutoRepeatMode {
    match mode {
        LoopMode::None => MediaPlaybackAutoRepeatMode::None,
        LoopMode::Track => MediaPlaybackAutoRepeatMode::Track,
        LoopMode::List => MediaPlaybackAutoRepeatMode::List,
    }
}

fn time_span(duration: Duration) -> TimeSpan {
    let ticks = (duration.as_nanos() / 100).min(i64::MAX as u128) as i64;
    TimeSpan { Duration: ticks }
}

fn duration_from_time_span(value: TimeSpan) -> Duration {
    if value.Duration <= 0 {
        return Duration::ZERO;
    }
    let nanos = (value.Duration as u128).saturating_mul(100);
    Duration::from_nanos(nanos.min(u64::MAX as u128) as u64)
}
