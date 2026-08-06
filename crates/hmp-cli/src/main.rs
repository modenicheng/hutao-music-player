//! HMP 命令行入口（docs/PROJECT.md §20 第一个里程碑）。
//!
//! ```text
//! hmp login                # QQ 扫码登录并保存凭证
//! hmp search "歌曲名"      # 搜索歌曲
//! hmp play <track-id>      # 播放（音质自动回退，需登录）
//! ```

use clap::{Parser, Subcommand};

mod credential_store;
mod login;
mod play;
mod search;

/// HMP 命令行客户端。
#[derive(Parser)]
#[command(name = "hmp", version, about = "胡桃音乐播放器命令行客户端")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// QQ 扫码登录并保存凭证。
    Login,
    /// 搜索歌曲。
    Search {
        /// 搜索关键词。
        keyword: String,
    },
    /// 播放歌曲（音质自动回退，需登录）。
    Play {
        /// 歌曲 track-id（songmid）。
        track_id: String,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    let cli = Cli::parse();
    let result = match cli.command {
        Command::Login => login::run().await,
        Command::Search { keyword } => search::run(&keyword).await,
        Command::Play { track_id } => play::run(&track_id).await,
    };

    if let Err(e) = result {
        eprintln!("错误: {e}");
        std::process::exit(1);
    }
}
