//! HMP 后台播放后端（docs/PROJECT.md §8.5）。
pub mod comment;
pub mod daemon;
pub mod engine;
pub mod local;
#[cfg(feature = "mpris")]
pub mod mpris;
pub mod player;
pub mod reconcile;
pub mod serve;
pub mod server; // Task 3 // Task 5
pub mod sync;
#[cfg(feature = "tray")]
pub mod tray; // Task 6
