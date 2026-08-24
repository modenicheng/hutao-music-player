//! Shared desktop application orchestration.
//!
//! This crate owns the single frontend-facing `AppCore` used by both the
//! legacy Slint desktop and the GPUI prototype. It contains no UI toolkit.

pub mod app;
pub mod lyrics;

pub use app::{
    AppCommand, AppCore, AppEvent, ThemeMode, UiFeatureData, UiLyricData, UiPage, UiQueueData,
    UiSongData,
};
pub use lyrics::parse_lrc;
