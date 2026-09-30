//! `hmp lyric`：歌词读取（走 daemon LyricGet；本地优先 + QQ 检索兜底）。
//!
//! ```text
//! hmp lyric <mid>                              # QQ 曲目按 mid 直取
//! hmp lyric local:<path>                       # 本地曲（同目录 .lrc / 内嵌标签）
//! hmp lyric local:<path> --title T --artist A  # 本地无词时按标题+歌手检索 QQ 兜底
//! ```

use std::io::Write;

use hmp_core::{LyricPage, Request, Response};

use super::client::DaemonClient;
use super::commands;
use super::output;

/// 读取并打印歌词。
pub async fn run(
    id: &str,
    title: &str,
    artist: &str,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    let resp = commands::send(
        &mut c,
        Request::LyricGet {
            id: id.to_string(),
            title: title.to_string(),
            artist: artist.to_string(),
        },
    )
    .await?;
    match resp {
        Response::Lyric(page) => {
            if json {
                return output::print(&page);
            }
            print_lyric(&page)
        }
        Response::Err { code, message } => {
            Err(format!("query failed ({code:?}): {message}").into())
        }
        _ => Err("unexpected lyric response".into()),
    }
}

fn print_lyric(page: &LyricPage) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    if page.lyric.is_empty() {
        writeln!(out, "No lyrics")?;
        return Ok(());
    }
    let source = match page.source.as_str() {
        "local" => "local (.lrc / embedded tag)",
        "qq" => "QQ Music",
        _ => "unknown",
    };
    writeln!(out, "[source: {source}]")?;
    write!(out, "{}", page.lyric)?;
    if !page.lyric.ends_with('\n') {
        writeln!(out)?;
    }
    if !page.translation.is_empty() {
        writeln!(out, "--- translation ---")?;
        write!(out, "{}", page.translation)?;
        if !page.translation.ends_with('\n') {
            writeln!(out)?;
        }
    }
    Ok(())
}
