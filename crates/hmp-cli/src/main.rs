//! HMP 命令行入口（docs/PROJECT.md §20 里程碑 + 后台播放 spec）。
//!
//! ```text
//! hmp login                  # QQ 扫码登录并保存凭证
//! hmp auth                   # 显示登录状况
//! hmp search "歌曲名"        # 搜索歌曲
//! hmp play <source>          # 遥控后端播放（track-id | playlist:<id> | album:<id>）
//! hmp status                 # 查询后端状态
//! hmp serve [--background]   # 前台/后台运行后端
//! ```

use clap::{Parser, Subcommand};

mod account;
mod auth;
mod client;
mod commands;
mod comment;
mod favorite;
mod history;
mod library;
mod login;
mod playlist;
mod quality;
mod scan;
mod search;

use hmp_core::{LoopMode, Request};

/// HMP 命令行客户端。
#[derive(Parser)]
#[command(
    name = "hmp",
    version,
    about = "Hutao Music Player command-line client"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// 顶层命令：高频短命令保留为 alias，完整命令面在二级子命令下。
#[derive(Subcommand)]
enum Command {
    // —— 高频 alias（保留）——
    /// Play (track / playlist:<id> / album:<id>; controls the daemon).
    Play { source: String },
    /// Play next (insert after current and play).
    PlayNext { source: String },
    /// Pause playback.
    Pause,
    /// Resume playback.
    Resume,
    /// Next track.
    Next,
    /// Previous track.
    Prev,
    /// Stop playback.
    Stop,
    /// Seek to position (seconds).
    Seek { secs: u64 },
    /// Set volume (0..1).
    Volume { value: f64 },
    /// Show daemon status.
    Status,
    /// Quit the daemon.
    Quit,
    /// Run the daemon in the foreground (daemon auto-spawn uses --background).
    Serve {
        /// Run in the background (detached from the terminal).
        #[arg(long)]
        background: bool,
    },
    /// Search songs.
    Search { keyword: String },
    /// Log in via QQ QR code (ASCII art in the terminal).
    Login,
    /// Show login status (local credential check).
    Auth,
    /// Recursively scan a local music directory into the library.
    Scan { dir: String },
    /// Local favorites: add / remove / list (reads the library directly).
    #[command(subcommand)]
    Favorite(FavoriteCmd),

    // —— 二级命令面 ——
    /// Player control.
    #[command(subcommand)]
    Player(PlayerCmd),
    /// Queue management.
    #[command(subcommand)]
    Queue(QueueCmd),
    /// Playlist management.
    #[command(subcommand)]
    Playlist(PlaylistCmd),
    /// Library queries and QQ sync.
    #[command(subcommand)]
    Library(LibraryCmd),
    /// QQ account info.
    #[command(subcommand)]
    Account(AccountCmd),
    /// Comments (list/post/reply/delete).
    #[command(subcommand)]
    Comment(CommentCmd),
}

/// `hmp player` 子命令。
#[derive(Subcommand)]
enum PlayerCmd {
    /// Show daemon status.
    Status,
    /// Pause playback.
    Pause,
    /// Resume playback.
    Resume,
    /// Next track.
    Next,
    /// Previous track.
    Prev,
    /// Stop playback.
    Stop,
    /// Seek to position (seconds).
    Seek { secs: u64 },
    /// Set volume (0..1).
    Volume { value: f64 },
    /// Quality policy: no arg = show; auto|master|hires|atmos|flac|aac|320|128 to set.
    Quality {
        /// Quality alias (omit to show current policy).
        alias: Option<String>,
        /// Disable fallback to lower tiers (try the requested tier only).
        #[arg(long)]
        no_fallback: bool,
    },
}

/// `hmp queue` 子命令。
#[derive(Subcommand)]
enum QueueCmd {
    /// List the queue (paged; title/artist projected via the local library).
    List {
        /// All pages (auto-paging, 50 per page by default).
        #[arg(long)]
        all: bool,
        /// Page size (default 50).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Alias for list.
    Show,
    /// Append to the end of the queue (without playing).
    Add { source: String },
    /// Insert after the current track and play it now.
    PlayNext { source: String },
    /// Remove the track at a 0-based index.
    Remove { index: usize },
    /// Clear the queue: keeps the current track by default; --all clears and stops.
    Clear {
        /// Also clear the current track (and stop playback).
        #[arg(long)]
        all: bool,
    },
    /// Shuffle: on / off.
    Shuffle { value: String },
    /// Loop mode: none / list / track.
    Loop { mode: String },
}

/// `hmp playlist` 子命令。
#[derive(Subcommand)]
enum PlaylistCmd {
    /// List playlists (--scope all|local|owned|favorite, default all).
    List {
        /// Scope: all | local | owned | favorite.
        #[arg(long)]
        scope: Option<String>,
    },
    /// Show tracks in a playlist.
    Show { id: i64 },
    /// Create a playlist.
    Create { name: String },
    /// Rename a playlist.
    Rename { id: i64, name: String },
    /// Add a track (QQ mid or local:<path>).
    Add { id: i64, track: String },
    /// Remove a track by position.
    Remove { id: i64, position: i64 },
    /// Delete a playlist.
    Delete { id: i64 },
}

/// `hmp library` 子命令。
#[derive(Subcommand)]
enum LibraryCmd {
    /// Recently played (reads the library directly).
    History { count: Option<u32> },
    /// Reconcile a snapshot of your QQ library (login required).
    Sync,
    /// Pending sync intents/errors (reads the library directly).
    SyncStatus,
    /// Browse local tracks (all local tracks by default).
    Tracks {
        /// Search (substring in title/artist/album).
        #[arg(long)]
        search: Option<String>,
        /// Filter by artist (matches any artist).
        #[arg(long)]
        artist: Option<String>,
        /// Filter by album.
        #[arg(long)]
        album: Option<String>,
        /// Show liked only.
        #[arg(long)]
        liked: bool,
    },
    /// Local album aggregation.
    Albums {
        /// Filter by album name substring.
        #[arg(long)]
        search: Option<String>,
        /// Show liked only (legacy entry).
        #[arg(long)]
        liked: bool,
    },
    /// Local artist aggregation.
    Artists,
    /// Scan a local directory into the library (registers it as a scan root).
    Scan { dir: String },
}

/// `hmp account` 子命令。
#[derive(Subcommand)]
enum AccountCmd {
    /// Profile header (nickname etc.).
    Profile,
    /// VIP info.
    Vip,
}

/// `hmp comment` 子命令。
#[derive(Subcommand)]
enum CommentCmd {
    /// List comments.
    List {
        /// Track mid.
        mid: String,
        /// Sort: hot | new | recommend (default hot).
        #[arg(long, default_value = "hot")]
        sort: String,
    },
    /// Post a comment.
    Post {
        /// Track mid.
        mid: String,
        /// Comment text.
        text: String,
    },
    /// Reply to a comment.
    Reply {
        /// Track mid.
        mid: String,
        /// Comment id to reply to.
        cm_id: String,
        /// Reply text.
        text: String,
    },
    /// Delete a comment.
    Delete {
        /// Comment id.
        cm_id: String,
    },
}

/// `hmp favorite` 子命令。
#[derive(Subcommand)]
enum FavoriteCmd {
    /// Like a track (QQ mid or local:<path>).
    Add { id: String },
    /// Unlike a track.
    Remove { id: String },
    /// List liked tracks.
    List,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// 分发子命令到本地逻辑或远端后端。
async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        // —— 高频 alias ——
        Command::Play { source } => {
            let mut c = client::DaemonClient::connect_or_spawn().await?;
            commands::cmd_play(&mut c, &source).await?;
            Ok(())
        }
        Command::PlayNext { source } => {
            let mut c = client::DaemonClient::connect_or_spawn().await?;
            commands::cmd_playnext(&mut c, &source).await?;
            Ok(())
        }
        Command::Pause => run_remote(commands::pause_req()).await,
        Command::Resume => run_remote(commands::resume_req()).await,
        Command::Next => run_remote(commands::next_req()).await,
        Command::Prev => run_remote(commands::prev_req()).await,
        Command::Stop => run_remote(commands::stop_req()).await,
        Command::Seek { secs } => run_remote(commands::seek_req(secs)).await,
        Command::Volume { value } => run_remote(commands::volume_req(value)).await,
        Command::Status => {
            let mut c = client::DaemonClient::connect_or_spawn().await?;
            commands::cmd_status(&mut c).await?;
            Ok(())
        }
        Command::Quit => run_remote(commands::quit_req()).await,
        Command::Serve { background } => {
            if background {
                hmp_daemon::serve::run_background().await
            } else {
                hmp_daemon::serve::run_foreground().await
            }
        }
        Command::Search { keyword } => search::run(&keyword).await,
        Command::Login => login::run().await,
        Command::Auth => auth::run().await,
        Command::Scan { dir } => scan::run(&dir).await,
        Command::Favorite(cmd) => match cmd {
            FavoriteCmd::Add { id } => favorite::add(&id).await,
            FavoriteCmd::Remove { id } => favorite::remove(&id).await,
            FavoriteCmd::List => favorite::list().await,
        },

        // —— 二级命令面 ——
        Command::Player(cmd) => match cmd {
            PlayerCmd::Status => {
                let mut c = client::DaemonClient::connect_or_spawn().await?;
                commands::cmd_status(&mut c).await?;
                Ok(())
            }
            PlayerCmd::Pause => run_remote(commands::pause_req()).await,
            PlayerCmd::Resume => run_remote(commands::resume_req()).await,
            PlayerCmd::Next => run_remote(commands::next_req()).await,
            PlayerCmd::Prev => run_remote(commands::prev_req()).await,
            PlayerCmd::Stop => run_remote(commands::stop_req()).await,
            PlayerCmd::Seek { secs } => run_remote(commands::seek_req(secs)).await,
            PlayerCmd::Volume { value } => run_remote(commands::volume_req(value)).await,
            PlayerCmd::Quality { alias, no_fallback } => quality::run(alias, no_fallback).await,
        },
        Command::Queue(cmd) => match cmd {
            QueueCmd::List { all, limit } => {
                let mut c = client::DaemonClient::connect_or_spawn().await?;
                commands::cmd_queue_list(&mut c, all, limit.unwrap_or(50)).await?;
                Ok(())
            }
            QueueCmd::Show => {
                let mut c = client::DaemonClient::connect_or_spawn().await?;
                commands::cmd_queue_list(&mut c, false, 50).await?;
                Ok(())
            }
            QueueCmd::Add { source } => run_remote(commands::queue_append_req(&source)).await,
            QueueCmd::PlayNext { source } => {
                run_remote(commands::queue_playnext_req(&source)).await
            }
            QueueCmd::Remove { index } => run_remote(commands::queue_remove_req(index)).await,
            QueueCmd::Clear { all } => run_remote(commands::queue_clear_req(all)).await,
            QueueCmd::Shuffle { value } => {
                let b = parse_bool(&value)?;
                run_remote(commands::shuffle_req(b)).await
            }
            QueueCmd::Loop { mode } => {
                let m = parse_loop_mode(&mode)?;
                run_remote(commands::loop_req(m)).await
            }
        },
        Command::Playlist(cmd) => match cmd {
            PlaylistCmd::List { scope } => playlist::list(scope.as_deref()).await,
            PlaylistCmd::Show { id } => playlist::show(id).await,
            PlaylistCmd::Create { name } => playlist::create(&name).await,
            PlaylistCmd::Rename { id, name } => playlist::rename(id, &name).await,
            PlaylistCmd::Add { id, track } => playlist::add(id, &track).await,
            PlaylistCmd::Remove { id, position } => playlist::remove_track(id, position).await,
            PlaylistCmd::Delete { id } => playlist::delete(id).await,
        },
        Command::Library(cmd) => match cmd {
            LibraryCmd::History { count } => history::run(count).await,
            LibraryCmd::Sync => library::sync().await,
            LibraryCmd::SyncStatus => library::sync_status().await,
            LibraryCmd::Tracks {
                search,
                artist,
                album,
                liked,
            } => {
                if liked && search.is_none() && artist.is_none() && album.is_none() {
                    library::tracks_liked().await
                } else {
                    library::tracks_local(
                        search.as_deref(),
                        artist.as_deref(),
                        album.as_deref(),
                        liked,
                    )
                    .await
                }
            }
            LibraryCmd::Albums { search, liked } => {
                if liked && search.is_none() {
                    library::albums_liked().await
                } else {
                    library::albums_local(search.as_deref()).await
                }
            }
            LibraryCmd::Artists => library::artists_local().await,
            LibraryCmd::Scan { dir } => scan::run(&dir).await,
        },
        Command::Account(cmd) => match cmd {
            AccountCmd::Profile => account::profile().await,
            AccountCmd::Vip => account::vip().await,
        },
        Command::Comment(cmd) => match cmd {
            CommentCmd::List { mid, sort } => comment::list(&mid, &sort).await,
            CommentCmd::Post { mid, text } => comment::post(&mid, &text).await,
            CommentCmd::Reply { mid, cm_id, text } => comment::reply(&mid, &cm_id, &text).await,
            CommentCmd::Delete { cm_id } => comment::delete(&cm_id).await,
        },
    }
}

/// 连接（必要时拉起）后端并发送一条简单命令。
async fn run_remote(command: impl Into<Request>) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = client::DaemonClient::connect_or_spawn().await?;
    commands::cmd_simple(&mut client, command.into()).await?;
    Ok(())
}

/// 解析循环模式字符串。
fn parse_loop_mode(s: &str) -> Result<LoopMode, Box<dyn std::error::Error>> {
    match s {
        "none" => Ok(LoopMode::None),
        "list" => Ok(LoopMode::List),
        "track" => Ok(LoopMode::Track),
        _ => Err(format!("unknown loop mode: {s} (none / list / track)").into()),
    }
}

/// 解析 on/off 布尔字符串。
fn parse_bool(s: &str) -> Result<bool, Box<dyn std::error::Error>> {
    match s {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("unknown value: {s} (on / off)").into()),
    }
}
