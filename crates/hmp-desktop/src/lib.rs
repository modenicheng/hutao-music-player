//! HMP 桌面应用库（Slint UI + mock 数据桥；供集成测试复用）。

slint::include_modules!();

pub mod app;
pub mod backend;
pub mod bridge;
pub mod covers;
pub mod format;
pub mod library_view;
pub mod lyrics;
pub mod mock;
pub mod player_bridge;
pub mod prefs;

pub use app::{
    AppCommand, AppCore, AppEvent, ThemeMode, UiFeatureData, UiLyricData, UiPage, UiQueueData,
    UiSongData,
};
pub use lyrics::parse_lrc;
