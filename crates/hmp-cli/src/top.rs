//! `hmp top`：排行榜（分类 + 详情；免登录，走 daemon）。
//!
//! ```text
//! hmp top                     # 分类视图（各榜预览前 3 首）
//! hmp top <top-id> [--page N] # 榜单详情曲目
//! ```

use hmp_core::{Request, Response};

use super::client::DaemonClient;
use super::commands;

/// 排行榜分类。
pub async fn category() -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    let resp = commands::send(&mut c, Request::TopCategoryGet).await?;
    match resp {
        Response::TopCategory(page) => print_category(&page),
        Response::Err { code, message } => {
            Err(format!("query failed ({code:?}): {message}").into())
        }
        _ => Err("unexpected top response".into()),
    }
}

/// 榜单详情。
pub async fn detail(top_id: i64, page: i64) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    let resp = commands::send(
        &mut c,
        Request::TopDetailGet {
            top_id,
            num: 100,
            page: page.max(1),
        },
    )
    .await?;
    match resp {
        Response::TopDetail(page) => print_detail(&page),
        Response::Err { code, message } => {
            Err(format!("query failed ({code:?}): {message}").into())
        }
        _ => Err("unexpected top response".into()),
    }
}

fn print_category(page: &hmp_core::TopCategoryPage) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    use std::io::Write;
    if page.groups.is_empty() {
        writeln!(out, "No toplist (network offline?)")?;
        return Ok(());
    }
    for g in &page.groups {
        writeln!(out, "== {} ==", g.name)?;
        for t in &g.tops {
            writeln!(
                out,
                "[{}] {}{}  (更新: {} · 播放 {})",
                t.id,
                t.name,
                if t.title_sub.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", t.title_sub)
                },
                if t.update_time.is_empty() { "-" } else { &t.update_time },
                t.listen_num
            )?;
            for line in &t.preview {
                writeln!(out, "     {line}")?;
            }
        }
        writeln!(out)?;
    }
    writeln!(out, "Detail: hmp top <top-id>")?;
    Ok(())
}

fn print_detail(page: &hmp_core::TopDetailPage) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    use std::io::Write;
    writeln!(
        out,
        "== {}{} == ({} 首 · 更新: {})",
        page.name,
        if page.title_sub.is_empty() {
            String::new()
        } else {
            format!(" · {}", page.title_sub)
        },
        page.total,
        if page.update_time.is_empty() { "-" } else { &page.update_time }
    )?;
    if page.songs.is_empty() {
        writeln!(out, "No songs (network offline?)")?;
        return Ok(());
    }
    for (i, s) in page.songs.iter().enumerate() {
        writeln!(
            out,
            "{:>3}. {} - {}  [{}]",
            i + 1,
            s.name,
            if s.singer.is_empty() { "-" } else { &s.singer },
            s.mid
        )?;
    }
    if page.has_more {
        writeln!(out, "（更多：--page 下一页）")?;
    }
    writeln!(out)?;
    writeln!(out, "Play: hmp play <songmid>")?;
    Ok(())
}
