//! HMP MPRIS 服务（docs/PROJECT.md §5.2 `hmp-mpris`）。
//!
//! 注册 `org.mpris.MediaPlayer2.hmp`，将系统媒体控制（playerctl/面板）
//! 转换为 [`hmp_core::PlayerCommand`]，并把 [`hmp_core::PlaybackState`]
//! 发布为 MPRIS 属性。
//!
//! 单一状态源：MPRIS 只消费 `PlaybackState`（watch），不自行推算进度。

pub mod metadata;
pub mod service;

pub use service::{MprisError, MprisService};
