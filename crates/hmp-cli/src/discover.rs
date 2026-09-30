//! `hmp discover`：发现页（推荐歌单 + 新歌；免登录，走 daemon）。
//!
//! ```text
//! hmp discover [--page N] [--area内地|欧美|日本|韩国|最新|港台]
//! ```

use hmp_core::{Request, Response};

use super::client::DaemonClient;
use super::commands;
use super::output;

/// 发现页。
pub async fn run(page: u32, area: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let area_code = area_code(area)?;
    let mut c = DaemonClient::connect_or_spawn().await?;
    let resp = commands::send(
        &mut c,
        Request::DiscoverGet {
            songlist_page: page.max(1),
            new_song_type: area_code,
        },
    )
    .await?;
    match resp {
        Response::Discover(page) => {
            if json {
                return output::print(&page);
            }
            print_page(&page)
        }
        Response::Err { code, message } => {
            Err(format!("query failed ({code:?}): {message}").into())
        }
        _ => Err("unexpected discover response".into()),
    }
}

/// 地区名 → 新歌接口 type 码（1=内地 2=欧美 3=日本 4=韩国 5=最新 6=港台）。
fn area_code(area: &str) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(match area {
        "内地" | "inland" => 1,
        "欧美" | "western" => 2,
        "日本" | "japan" => 3,
        "韩国" | "korea" => 4,
        "最新" | "new" => 5,
        "港台" | "hktw" => 6,
        other => return Err(format!("unknown area: {other}").into()),
    })
}

fn print_page(page: &hmp_core::DiscoverPage) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    use std::io::Write;

    if !page.playlists.is_empty() {
        writeln!(out, "== 推荐歌单（{}）==", page.playlists.len())?;
        for (i, p) in page.playlists.iter().enumerate() {
            writeln!(
                out,
                "{:>3}. [{}] {}  [{}首 · {}]",
                i + 1,
                p.id,
                p.title,
                p.songnum,
                if p.creator.is_empty() { "-" } else { &p.creator }
            )?;
        }
        if page.has_more_playlists {
            writeln!(out, "（更多歌单：--page 下一页）")?;
        }
        writeln!(out)?;
        writeln!(out, "Play: hmp play playlist:<id>")?;
    }

    if !page.new_songs.is_empty() {
        writeln!(out)?;
        writeln!(out, "== 新歌速递（{}）==", page.new_songs.len())?;
        for (i, s) in page.new_songs.iter().enumerate() {
            writeln!(
                out,
                "{:>3}. {} - {}  [{}]",
                i + 1,
                s.name,
                if s.singer.is_empty() { "-" } else { &s.singer },
                s.mid
            )?;
        }
        writeln!(out)?;
        writeln!(out, "Play: hmp play <songmid>")?;
    }

    if page.playlists.is_empty() && page.new_songs.is_empty() {
        writeln!(out, "No content (network offline?)")?;
    }
    Ok(())
}
