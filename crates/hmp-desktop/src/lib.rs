//! HMP 桌面应用库（Slint UI + IPC/媒体库数据桥；供集成测试复用）。

slint::include_modules!();

pub mod backend;
pub mod bridge;
pub mod cover_cache;
pub mod covers;
pub mod format;
pub mod library_view;
pub mod lyrics;
pub mod online_covers;
pub mod player_bridge;
pub mod prefs;
pub mod track_theme;

pub use lyrics::parse_lrc;
