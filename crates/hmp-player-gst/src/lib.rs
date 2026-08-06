//! HMP GStreamer 播放器核心（docs/PROJECT.md §5.2 `hmp-player-gst`）。
//!
//! 职责：URI 加载、播放/暂停/停止、Seek、音量、缓冲、错误/EOS 处理、
//! 状态事件转换。**不直接调用 QQ API**——曲目元数据与 URI 由调用方
//! （应用层）解析后传入 [`PlayerCore`]。

pub mod core;
pub mod events;

pub use core::{LoadRequest, PlayerCore};
pub use events::PlayerEvent;
