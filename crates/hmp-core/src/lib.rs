//! HMP 核心领域模型（docs/PROJECT.md §5.2 `hmp-core`）。
//!
//! 只存放稳定领域模型与应用层协议，**不得依赖** Slint、GStreamer、SQLite
//! 或具体 QQ 接口字段：
//!
//! - [`media`]：`Track` / `ArtistRef` / `AlbumRef` / `Playlist` / [`AudioQuality`]
//! - [`player`]：`PlayerCommand` / `PlaybackState` / `LoopMode`
//! - [`auth`]：`CredentialSummary`
//! - [`error`]：核心错误分类 [`HmpError`]
//!
//! 标识符一律使用 newtype（[`id`]），禁止跨模块传递裸 `String`。

pub mod auth;
pub mod error;
pub mod id;
pub mod ipc;
pub mod media;
pub mod player;
pub mod queue;

pub use auth::CredentialSummary;
pub use error::HmpError;
pub use id::{AlbumId, ArtistId, PlaylistId, TrackId};
pub use ipc::{
    DaemonState, ErrorInfo, Event, IpcErrorCode, PlayRequest, Request, Response, TrackProvider,
    TrackRef,
};
pub use media::{Album, AlbumRef, ArtistRef, AudioQuality, CoverRef, Playlist, Track};
pub use player::{LoopMode, PlaybackCapabilities, PlaybackState, PlaybackStatus, PlayerCommand};
pub use queue::{QueueCore, QueueSnapshot};
