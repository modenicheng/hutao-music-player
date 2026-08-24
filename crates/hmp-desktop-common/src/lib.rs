//! Shared desktop application orchestration.
//!
//! This crate owns the single frontend-facing `AppCore` used by both the
//! legacy Slint desktop and the GPUI prototype. It contains no UI toolkit.

pub mod app;
pub mod lyrics;

pub use app::{
    AppCommand, AppCore, AppEvent, ThemeMode, UiAuthData, UiFeatureData, UiLoginPhase, UiLyricData,
    UiPage, UiPlaylistData, UiPlaylistTrackData, UiQueueData, UiSongData,
};
pub use lyrics::parse_lrc;
