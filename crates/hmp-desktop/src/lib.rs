//! HMP 桌面应用库（UI 桥接逻辑，供集成测试复用）。

slint::include_modules!();

pub mod app;
pub mod bridge;
pub mod demo;
pub mod lyrics;

pub use app::{
    AppCommand, AppCore, AppEvent, ThemeMode, UiFeatureData, UiLyricData, UiPage, UiQueueData,
    UiSongData,
};
pub use demo::UiLibraryData;
pub use lyrics::parse_lrc;

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod ui_bridge_integration;
