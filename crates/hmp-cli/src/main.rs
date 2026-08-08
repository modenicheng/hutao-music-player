//! HMP 命令行入口（docs/PROJECT.md §20 里程碑 + 后台播放 spec）。
//!
//! ```text
//! hmp login                  # QQ 扫码登录并保存凭证
//! hmp search "歌曲名"        # 搜索歌曲
//! hmp play <source>          # 遥控后端播放（track-id | playlist:<id> | album:<id>）
//! hmp status                 # 查询后端状态
//! hmp serve [--background]   # 前台/后台运行后端
//! ```

use clap::{Parser, Subcommand};

mod client;
mod commands;
mod login;
mod search;

use hmp_core::{LoopMode, Request};

/// HMP 命令行客户端。
#[derive(Parser)]
#[command(name = "hmp", version, about = "胡桃音乐播放器命令行客户端")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// QQ 扫码登录（终端 ASCII 二维码）。
    Login,
    /// 搜索歌曲。
    Search { keyword: String },
    /// 播放（单曲 / playlist:<id> / album:<id>；遥控后端）。
    Play { source: String },
    /// 插队播放。
    PlayNext { source: String },
    /// 队列管理：show / add <id> / remove <idx> / clear。
    Queue { args: Vec<String> },
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
    /// 循环模式：none / list / track。
    Loop { mode: String },
    /// 随机播放：on / off。
    Shuffle { value: String },
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
        Command::Login => login::run().await,
        Command::Search { keyword } => search::run(&keyword).await,
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
        Command::Queue { args } => {
            let mut c = client::DaemonClient::connect_or_spawn().await?;
            commands::cmd_queue(&mut c, &args).await?;
            Ok(())
        }
        Command::Pause => run_remote(commands::pause_req()).await,
        Command::Resume => run_remote(commands::resume_req()).await,
        Command::Next => run_remote(commands::next_req()).await,
        Command::Prev => run_remote(commands::prev_req()).await,
        Command::Stop => run_remote(commands::stop_req()).await,
        Command::Seek { secs } => run_remote(commands::seek_req(secs)).await,
        Command::Volume { value } => run_remote(commands::volume_req(value)).await,
        Command::Loop { mode } => {
            let m = parse_loop_mode(&mode)?;
            run_remote(commands::loop_req(m)).await
        }
        Command::Shuffle { value } => {
            let b = parse_bool(&value)?;
            run_remote(commands::shuffle_req(b)).await
        }
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
