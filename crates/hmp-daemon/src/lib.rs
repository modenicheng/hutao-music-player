//! HMP 后台播放后端（docs/PROJECT.md §8.5）。

/// daemon 构建指纹（build.rs 对 `src/**.rs` + `Cargo.toml` 的 FNV-1a，变化 ⇔
/// daemon 代码变化）。桌面端 `connect_or_spawn` 以 IPC `DaemonState.backend_build`
/// 与本值比对，不一致 = 运行中 daemon 是陈旧构建 → 自动重启后端
/// （AUDIT §16 零号发现 / §18「陈旧常驻 daemon」复发陷阱的收口）。
pub const BUILD_CODE: &str = env!("HMP_BUILD_CODE_HASH");

pub mod comment;
pub mod content;
pub mod daemon;
pub mod engine;
pub mod local;
pub mod login;
#[cfg(feature = "mpris")]
pub mod mpris;
pub mod player;
pub mod reconcile;
pub mod serve;
pub mod server; // Task 3 // Task 5
#[cfg(windows)]
pub mod smtc;
pub mod sync;
pub mod transport;
#[cfg(feature = "tray")]
pub mod tray; // Task 6
pub mod watcher;
