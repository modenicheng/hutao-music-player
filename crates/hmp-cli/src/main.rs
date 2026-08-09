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
#[command(name = "hmp", version, about = "胡桃音乐播放器命令行客户端")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// 顶层命令：高频短命令保留为 alias，完整命令面在二级子命令下。
#[derive(Subcommand)]
enum Command {
    // —— 高频 alias（保留）——
    /// 播放（单曲 / playlist:<id> / album:<id>；遥控后端）。
    Play { source: String },
    /// 插队播放。
    PlayNext { source: String },
    /// 暂停。
    Pause,
    /// 继续播放。
    Resume,
    /// 下一首。
    Next,
    /// 上一首。
    Prev,
    /// 停止。
    Stop,
    /// 跳转（秒）。
    Seek { secs: u64 },
    /// 音量（0..1）。
    Volume { value: f64 },
    /// 查询状态。
    Status,
    /// 退出后端。
    Quit,
    /// 前台运行后端（--background 由 CLI 自动拉起使用）。
    Serve {
        /// 后台模式（脱离终端）。
        #[arg(long)]
        background: bool,
    },
    /// 搜索歌曲。
    Search { keyword: String },
    /// QQ 扫码登录（终端 ASCII 二维码）。
    Login,
    /// 显示登录状况（本地凭证检查）。
    Auth,
    /// 递归扫描本地音乐目录入库。
    Scan { dir: String },
    /// 本地收藏管理：add / remove / list（直读媒体库）。
    #[command(subcommand)]
    Favorite(FavoriteCmd),

    // —— 二级命令面 ——
    /// 播放器控制。
    #[command(subcommand)]
    Player(PlayerCmd),
    /// 队列管理。
    #[command(subcommand)]
    Queue(QueueCmd),
    /// 本地歌单管理。
    #[command(subcommand)]
    Playlist(PlaylistCmd),
    /// 媒体库查询与 QQ 同步。
    #[command(subcommand)]
    Library(LibraryCmd),
    /// QQ 账号信息。
    #[command(subcommand)]
    Account(AccountCmd),
    /// 评论（list/post/reply/delete）。
    #[command(subcommand)]
    Comment(CommentCmd),
}

/// `hmp player` 子命令。
#[derive(Subcommand)]
enum PlayerCmd {
    /// 查询状态。
    Status,
    /// 暂停。
    Pause,
    /// 继续播放。
    Resume,
    /// 下一首。
    Next,
    /// 上一首。
    Prev,
    /// 停止。
    Stop,
    /// 跳转（秒）。
    Seek { secs: u64 },
    /// 音量（0..1）。
    Volume { value: f64 },
    /// 音质策略：无参显示；auto|master|hires|atmos|flac|aac|320|128 设置。
    Quality {
        /// 音质别名（缺省 = 仅显示）。
        alias: Option<String>,
        /// 禁止向下降级回退（仅尝试指定档位）。
        #[arg(long)]
        no_fallback: bool,
    },
}

/// `hmp queue` 子命令。
#[derive(Subcommand)]
enum QueueCmd {
    /// 列出队列（分页；标题/歌手经本地媒体库投影）。
    List {
        /// 全部（自动翻页，默认 50/页）。
        #[arg(long)]
        all: bool,
        /// 页大小（默认 50）。
        #[arg(long)]
        limit: Option<usize>,
    },
    /// list 别名。
    Show,
    /// 追加到队尾（不播放）。
    Add { source: String },
    /// 插到当前曲之后并立即播放。
    PlayNext { source: String },
    /// 移除 0 基位置曲目。
    Remove { index: usize },
    /// 清空队列：默认保留当前曲；--all 清空并停止。
    Clear {
        /// 连当前曲一起清空（并停止播放）。
        #[arg(long)]
        all: bool,
    },
    /// 随机播放：on / off。
    Shuffle { value: String },
    /// 循环模式：none / list / track。
    Loop { mode: String },
}

/// `hmp playlist` 子命令。
#[derive(Subcommand)]
enum PlaylistCmd {
    /// 列出歌单（--scope all|local|owned|favorite，默认 all）。
    List {
        /// 范围：all | local | owned | favorite。
        #[arg(long)]
        scope: Option<String>,
    },
    /// 查看歌单内曲目。
    Show { id: i64 },
    /// 新建歌单。
    Create { name: String },
    /// 重命名。
    Rename { id: i64, name: String },
    /// 追加曲目（QQ mid 或 local:<path>）。
    Add { id: i64, track: String },
    /// 按序号移除曲目。
    Remove { id: i64, position: i64 },
    /// 删除歌单。
    Delete { id: i64 },
}

/// `hmp library` 子命令。
#[derive(Subcommand)]
enum LibraryCmd {
    /// 最近播放（直读媒体库）。
    History { count: Option<u32> },
    /// 从 QQ 拉用户库快照 reconcile（需登录）。
    Sync,
    /// 待同步意图/错误（直读媒体库）。
    SyncStatus,
    /// 我喜欢的歌曲（本地事实视图）。
    Tracks {
        /// 只看已收藏。
        #[arg(long)]
        liked: bool,
    },
    /// 我收藏的专辑（本地事实视图）。
    Albums {
        /// 只看已收藏。
        #[arg(long)]
        liked: bool,
    },
}

/// `hmp account` 子命令。
#[derive(Subcommand)]
enum AccountCmd {
    /// 主页头部（昵称等）。
    Profile,
    /// VIP 信息。
    Vip,
}

/// `hmp comment` 子命令。
#[derive(Subcommand)]
enum CommentCmd {
    /// 评论列表。
    List {
        /// 曲目 mid。
        mid: String,
        /// 排序：hot | new | recommend（默认 hot）。
        #[arg(long, default_value = "hot")]
        sort: String,
    },
    /// 发表评论。
    Post {
        /// 曲目 mid。
        mid: String,
        /// 评论内容。
        text: String,
    },
    /// 回复评论。
    Reply {
        /// 曲目 mid。
        mid: String,
        /// 被回复评论 id。
        cm_id: String,
        /// 回复内容。
        text: String,
    },
    /// 删除评论。
    Delete {
        /// 评论 id。
        cm_id: String,
    },
}

/// `hmp favorite` 子命令。
#[derive(Subcommand)]
enum FavoriteCmd {
    /// 收藏曲目（QQ mid 或 local:<path>）。
    Add { id: String },
    /// 取消收藏。
    Remove { id: String },
    /// 列出收藏。
    List,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("错误: {e}");
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
            LibraryCmd::Tracks { liked } => {
                if liked {
                    library::tracks_liked().await
                } else {
                    Err("hmp library tracks --liked".into())
                }
            }
            LibraryCmd::Albums { liked } => {
                if liked {
                    library::albums_liked().await
                } else {
                    Err("hmp library albums --liked".into())
                }
            }
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
        _ => Err(format!("未知循环模式: {s}（none / list / track）").into()),
    }
}

/// 解析 on/off 布尔字符串。
fn parse_bool(s: &str) -> Result<bool, Box<dyn std::error::Error>> {
    match s {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("未知取值: {s}（on / off）").into()),
    }
}
