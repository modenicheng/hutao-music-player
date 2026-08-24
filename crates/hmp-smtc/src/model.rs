//! Pure projection between HMP domain state and Windows media concepts.

use std::{path::PathBuf, time::Duration};

use hmp_core::{LoopMode, PlaybackCapabilities, PlaybackState, PlaybackStatus, PlayerCommand};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectedStatus {
    Playing,
    Paused,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectedButton {
    Play,
    Pause,
    Stop,
    Next,
    Previous,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub status: ProjectedStatus,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub position: Duration,
    pub duration: Option<Duration>,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_stop: bool,
    pub can_seek: bool,
    pub can_next: bool,
    pub can_previous: bool,
    pub loop_mode: LoopMode,
    pub shuffle: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CoverSource {
    File(PathBuf),
    Uri(String),
}

pub(crate) fn classify_cover_source(value: &str) -> Option<CoverSource> {
    let uri = url::Url::parse(value).ok()?;
    match uri.scheme() {
        "file" => uri.to_file_path().ok().map(CoverSource::File),
        "http" | "https" | "ms-appx" | "ms-appdata" => Some(CoverSource::Uri(value.to_owned())),
        _ => None,
    }
}

impl Projection {
    pub fn from_state(state: &PlaybackState, capabilities: PlaybackCapabilities) -> Self {
        let status = match state.status {
            PlaybackStatus::Playing => ProjectedStatus::Playing,
            PlaybackStatus::Paused => ProjectedStatus::Paused,
            PlaybackStatus::Empty
            | PlaybackStatus::Loading
            | PlaybackStatus::Buffering
            | PlaybackStatus::Stopped
            | PlaybackStatus::Ended
            | PlaybackStatus::Error => ProjectedStatus::Stopped,
        };
        let track = state.current.as_ref();
        let has_track = track.is_some();
        Self {
            status,
            title: track.map(|track| track.title.clone()),
            artist: track.and_then(|track| {
                let names = track
                    .artists
                    .iter()
                    .map(|artist| artist.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                (!names.is_empty()).then_some(names)
            }),
            album: track.and_then(|track| track.album.as_ref().map(|album| album.name.clone())),
            cover_url: track.and_then(|track| track.cover.as_ref().map(|cover| cover.url.clone())),
            position: state.position,
            duration: state.duration,
            can_play: has_track,
            can_pause: has_track,
            can_stop: has_track,
            can_seek: has_track && state.can_seek && state.duration.is_some(),
            can_next: capabilities.can_go_next,
            can_previous: capabilities.can_go_previous,
            loop_mode: state.loop_mode,
            shuffle: state.shuffle,
        }
    }

    /// Whether both projections describe the same shell-visible media item.
    ///
    /// Timeline and transport capability changes are intentionally excluded:
    /// rebuilding SMTC metadata for the 100 ms position tick repeatedly clears
    /// its asynchronously opened thumbnail stream.
    pub fn has_same_metadata(&self, other: &Self) -> bool {
        self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.cover_url == other.cover_url
    }
}

pub const fn map_button(button: ProjectedButton) -> Option<PlayerCommand> {
    Some(match button {
        ProjectedButton::Play => PlayerCommand::Play,
        ProjectedButton::Pause => PlayerCommand::Pause,
        ProjectedButton::Stop => PlayerCommand::Stop,
        ProjectedButton::Next => PlayerCommand::Next,
        ProjectedButton::Previous => PlayerCommand::Previous,
    })
}

pub const fn map_repeat_request(value: i32) -> Option<PlayerCommand> {
    let mode = match value {
        0 => LoopMode::None,
        1 => LoopMode::Track,
        2 => LoopMode::List,
        _ => return None,
    };
    Some(PlayerCommand::SetLoopMode(mode))
}
