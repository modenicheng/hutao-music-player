//! `hmp comment`：评论（走 daemon CommentService；spec §6）。
//!
//! ```text
//! hmp comment list <mid> [--sort hot|new|recommend]   # 评论列表（默认 hot）
//! hmp comment post <mid> <text>                       # 发表评论
//! hmp comment reply <mid> <comment-id> <text>         # 回复评论
//! hmp comment delete <comment-id>                     # 删除评论
//! ```

use std::io::Write;

use hmp_core::{CommentPage, Request, Response};

use super::client::DaemonClient;
use super::commands;

/// 评论列表（`page` 1 基页号；`num` 页大小 1..=100）。
pub async fn list(
    mid: &str,
    sort: &str,
    page: u32,
    num: u32,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(sort, "hot" | "new" | "recommend") {
        return Err(format!("unknown sort: {sort} (hot | new | recommend)").into());
    }
    let mut c = DaemonClient::connect_or_spawn().await?;
    let resp = commands::send(
        &mut c,
        Request::CommentList {
            mid: mid.to_string(),
            sort: sort.to_string(),
            page: page.max(1),
            num: num.clamp(1, 100),
        },
    )
    .await?;
    match resp {
        Response::CommentList(page) => {
            if json {
                return super::output::print(&page);
            }
            print_page(&page)
        }
        Response::Err { code, message } => {
            Err(format!("query failed ({code:?}): {message}").into())
        }
        _ => Err("unexpected comment response".into()),
    }
}

fn print_page(page: &CommentPage) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    if page.comments.is_empty() {
        writeln!(out, "No comments yet")?;
    } else {
        writeln!(
            out,
            "{} comment(s) (page {}, showing {}{})",
            page.total,
            page.page,
            page.comments.len(),
            if page.has_more {
                ", more pages"
            } else {
                ""
            }
        )?;
        for c in &page.comments {
            let time = format_time(c.time);
            writeln!(
                out,
                "[{}] {}  {}  ({} likes)",
                c.cm_id, c.nickname, time, c.like_count
            )?;
            writeln!(out, "   {}", c.content)?;
        }
        if page.has_more {
            writeln!(out, "（更多：--page {}）", page.page + 1)?;
        }
    }
    out.flush()?;
    Ok(())
}

/// unix 秒 → `YYYY-MM-DD HH:MM`（QQ 评论时间为 UTC+8 固定偏移）。
fn format_time(secs: i64) -> String {
    crate::timefmt::format_with_offset(secs, 8 * 3600).unwrap_or_else(|| secs.to_string())
}

/// 发表评论。
pub async fn post(mid: &str, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    commands::cmd_simple(
        &mut c,
        Request::CommentPost {
            mid: mid.to_string(),
            content: content.to_string(),
            reply_cmt_id: None,
        },
    )
    .await?;
    println!("Comment posted");
    Ok(())
}

/// 回复评论。
pub async fn reply(
    mid: &str,
    cm_id: &str,
    content: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    commands::cmd_simple(
        &mut c,
        Request::CommentPost {
            mid: mid.to_string(),
            content: content.to_string(),
            reply_cmt_id: Some(cm_id.to_string()),
        },
    )
    .await?;
    println!("Replied to {cm_id}");
    Ok(())
}

/// 删除评论。
pub async fn delete(cm_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    commands::cmd_simple(
        &mut c,
        Request::CommentDelete {
            cm_id: cm_id.to_string(),
        },
    )
    .await?;
    println!("Deleted comment {cm_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_time_utc8() {
        // 2026-08-09 00:00 UTC = 08:00 UTC+8
        let s = format_time(1_786_233_600);
        assert!(s.contains("2026-08-09 08:00"), "UTC+8 显示: {s}");
    }
}
