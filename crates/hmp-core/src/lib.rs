//! HMP 核心领域模型（docs/PROJECT.md §5.2 `hmp-core`）。
//!
//! 只存放稳定领域模型与应用层协议，**不得依赖** Slint、具体音频驱动、SQLite
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
    AccountInfo, CommentItem, CommentPage, DaemonState, DiscoverNewSong, DiscoverPage,
    DiscoverPlaylist, EnginePhase, ErrorInfo, Event, GuessPage, IpcErrorCode, LoginQrSession,
    LoginQrState, LyricPage, PlayRequest, PlaylistWriteOp, QualityPrefDto, QueueEntry, QueuePage,
    Request, Response, SearchAlbum, SearchPage, SearchSinger, SearchSong, TopCategoryPage,
    TopDetailPage, TopGroupDto, TopSongDto, TopSummaryDto, TrackProvider, TrackRef,
};
pub use media::{Album, AlbumRef, ArtistRef, AudioQuality, CoverRef, Playlist, Track, TrackStub};
pub use player::{
    LoadRequest, LoopMode, MediaStream, MediaStreamSource, PlaybackCapabilities, PlaybackState,
    PlaybackStatus, PlayerCommand, PlayerEvent,
};
pub use queue::{QueueCore, QueueSnapshot};
