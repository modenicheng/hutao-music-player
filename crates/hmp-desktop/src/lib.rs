//! HMP 桌面应用库（UI 桥接逻辑，供集成测试复用）。

slint::include_modules!();

pub mod app;
pub mod bridge;

pub use app::{
    AppCommand, AppCore, AppEvent, ThemeMode, UiFeatureData, UiLyricData, UiPage, UiQueueData,
    UiSongData,
};

#[cfg(test)]
mod bridge_tests;
