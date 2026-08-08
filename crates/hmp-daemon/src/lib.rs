//! HMP 后台播放后端（docs/PROJECT.md §8.5）。
pub mod daemon;
pub mod engine;
#[cfg(feature = "mpris")]
pub mod mpris;
pub mod player;
pub mod serve;
pub mod server; // Task 3 // Task 5
#[cfg(feature = "tray")]
pub mod tray; // Task 6
