//! # hmp-qqmusic-api
//!
//! 非官方 QQ 音乐（QQ Music）API 的 Rust 客户端。
//!
//! 本 crate 是 [L-1124/QQMusicApi](https://github.com/L-1124/QQMusicApi)（Python,
//! GPL-3.0-or-later）的 Rust 移植：以 Python 实现为行为规范与差分测试 Oracle，
//! 在 Rust 侧重新定义模块边界、错误类型与并发模型。
//!
//! 分层（docs/PROJECT.md §6.2）：
//!
//! ```text
//! QQ 原始请求/响应
//!         │
//!         ▼
//! wire DTO / serde_json::Value
//!         │
//!         ▼ normalize
//! 领域模型 Track / Album / Artist / Playlist
//!         │
//!         ▼
//! 应用核心 / UI / MPRIS
//! ```
//!
//! 本 crate 不依赖 UI、MPRIS 或播放器（docs/PROJECT.md §5.2）。

#![forbid(unsafe_code)]
#![warn(missing_docs)]
// 生产代码禁止 unwrap/expect（测试/示例中放行，属惯例用法）
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod album;
pub mod algorithms;
pub mod client;
pub mod config;
pub mod credential;
pub mod error;
pub mod login;
pub mod lyric;
pub mod models;
pub mod protocol;
pub mod recommend;
pub mod singer;
pub mod song;
pub mod songlist;
pub mod top;
pub mod user;

pub use album::{
    AlbumApi, AlbumFavWriteResponse, GetAlbumDetailResponse, GetAlbumSongResponse,
    GetNewAlbumResponse,
};
pub use client::QqMusicClient;
pub use config::ClientConfig;
pub use credential::Credential;
pub use error::QqMusicError;
pub use login::{LoginApi, QR, QRCodeLoginEvents, QRLoginResult, QRLoginType};
pub use lyric::{GetLyricResponse, LyricApi};
pub use recommend::{
    GuessRecommendResponse, RadarRecommendResponse, RecommendApi, RecommendFeedCardResponse,
    RecommendNewSongResponse, RecommendSonglistResponse,
};
pub use singer::{
    AlbumBrief, AreaType, GenreType, HomepageHeaderResponse, HomepageTabDetailResponse, IndexType,
    SexType, SimilarSingerResponse, SingerAlbumListResponse, SingerApi, SingerDetailResponse,
    SingerIndexPageResponse, SingerMvListResponse, SingerSongListResponse, SingerTypeListResponse,
    TabType, VideoBrief,
};
pub use song::{GetSongDetailResponse, GetSongUrlsResponse, SongApi, SongFileInfo, SongFileType};
pub use songlist::{CreateDeleteSonglistResp, GetSonglistDetailResponse, SonglistApi};
pub use top::{TopApi, TopCategoryResponse, TopDetailResponse};
pub use user::{
    UserApi, UserCreatedSonglistResponse, UserFavAlbumResponse, UserFavSonglistResponse,
};
