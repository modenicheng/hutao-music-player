//! HMP 桌面应用库（UI 桥接逻辑，供集成测试复用）。

slint::include_modules!();

pub mod bridge;
pub mod demo;

pub use hmp_desktop_common::{app, lyrics};

pub use demo::UiLibraryData;
pub use hmp_desktop_common::{
    AppCommand, AppCore, AppEvent, ThemeMode, UiFeatureData, UiLyricData, UiPage, UiQueueData,
    UiSongData, parse_lrc,
};

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod ui_bridge_integration;
