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

/// 评论列表。
pub async fn list(mid: &str, sort: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(sort, "hot" | "new" | "recommend") {
        return Err(format!("未知排序: {sort}（hot | new | recommend）").into());
    }
    let mut c = DaemonClient::connect_or_spawn().await?;
    let resp = commands::send(
        &mut c,
        Request::CommentList {
            mid: mid.to_string(),
            sort: sort.to_string(),
        },
    )
    .await?;
    match resp {
        Response::CommentList(page) => print_page(&page),
        Response::Err { code, message } => Err(format!("查询失败({code:?}): {message}").into()),
        _ => Err("评论响应异常".into()),
    }
}

fn print_page(page: &CommentPage) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    if page.comments.is_empty() {
        writeln!(out, "暂无评论")?;
    } else {
        writeln!(
            out,
            "共 {} 条评论（显示前 {} 条）",
            page.total,
            page.comments.len()
        )?;
        for c in &page.comments {
            let time = format_time(c.time);
            writeln!(
                out,
                "[{}] {}  {}  (赞 {})",
                c.cm_id, c.nickname, time, c.like_count
            )?;
            writeln!(out, "   {}", c.content)?;
        }
    }
    out.flush()?;
    Ok(())
}

/// unix 秒 → `YYYY-MM-DD HH:MM`。
fn format_time(secs: i64) -> String {
    let Some(dt) = chrono_lite(secs) else {
        return secs.to_string();
    };
    dt
}

/// 无 chrono 依赖的本地时间格式化（UTC+8 固定偏移，够用即可）。
fn chrono_lite(secs: i64) -> Option<String> {
    let secs = secs + 8 * 3600; // UTC+8
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m) = (rem / 3600, (rem % 3600) / 60);
    // 1970-01-01 起的天数 → 年月日（民用历法）。
    let (y, mth, d) = civil_from_days(days)?;
    Some(format!("{y:04}-{mth:02}-{d:02} {h:02}:{m:02}"))
}

/// 天数 → (年, 月, 日)：Howard Hinnant 算法。
fn civil_from_days(z: i64) -> Option<(i64, i64, i64)> {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some((y, m, d))
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
    println!("已发表评论");
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
    println!("已回复 {cm_id}");
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
    println!("已删除评论 {cm_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_conversion() {
        // 1970-01-01
        assert_eq!(civil_from_days(0), Some((1970, 1, 1)));
        // 2026-08-05 / 2026-08-09（Unix 天）
        assert_eq!(civil_from_days(20_670), Some((2026, 8, 5)));
        assert_eq!(civil_from_days(20_674), Some((2026, 8, 9)));
        // 闰年 2000-02-29（天 11016）
        assert_eq!(civil_from_days(11_016), Some((2000, 2, 29)));
    }

    #[test]
    fn format_time_utc8() {
        // 2026-08-09 00:00 UTC = 08:00 UTC+8
        let s = format_time(1_786_233_600);
        assert!(s.contains("2026-08-09 08:00"), "UTC+8 显示: {s}");
    }
}
