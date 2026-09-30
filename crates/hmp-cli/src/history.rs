//! `hmp history`：最近播放（直读媒体库；daemon 与 CLI 同机同文件，WAL 并发安全）。

use std::io::Write;

use hmp_storage::{LibraryDb, RecentPlay};

/// 格式化一条最近播放。
pub fn format_recent(r: &RecentPlay) -> String {
    let artist = r.artist.as_deref().unwrap_or("");
    let listened = r.listened_ms / 1000;
    let status = if r.ended_at.is_some() {
        format!("(listened {listened}s · {})", r.reason)
    } else {
        "(playing now)".to_string()
    };
    format!(
        "{:>2}. {} - {artist}  {status}  {}",
        r.track_id,
        r.title,
        fmt_time(r.started_at)
    )
}

/// 打印最近播放列表（默认 10 条）。
pub async fn run(limit: Option<u32>, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let path = hmp_storage::data_dir().join("library.sqlite3");
    if !path.exists() {
        eprintln!(
            "library not available (no play history yet): {}",
            path.display()
        );
        return Ok(());
    }
    let mut db = LibraryDb::open(&path)?;
    let plays = db.recent_plays(limit.unwrap_or(10))?;
    if json {
        let items: Vec<serde_json::Value> = plays
            .iter()
            .map(|r| {
                serde_json::json!({
                    "track_id": r.track_id,
                    "title": r.title,
                    "artist": r.artist,
                    "source": r.source,
                    "source_key": r.source_key,
                    "started_at": r.started_at,
                    "ended_at": r.ended_at,
                    "listened_ms": r.listened_ms,
                    "reason": r.reason,
                })
            })
            .collect();
        return super::output::print(&serde_json::json!({ "total": items.len(), "items": items }));
    }
    let mut stdout = std::io::stdout().lock();
    if plays.is_empty() {
        writeln!(stdout, "No play history yet")?;
    } else {
        for r in &plays {
            writeln!(stdout, "{}", format_recent(r))?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// unix 秒 → `YYYY-MM-DD HH:MM`（UTC；实现见 [`crate::timefmt`]）。
fn fmt_time(ts: i64) -> String {
    crate::timefmt::format_utc(ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_roundtrip_epoch() {
        // 1970-01-01 00:00 UTC
        assert_eq!(fmt_time(0), "1970-01-01 00:00");
        // 2026-08-08 12:00 UTC（由 unix 秒推算）
        let ts = 1_786_190_400; // 2026-08-08 12:00:00 UTC
        assert_eq!(fmt_time(ts), "2026-08-08 12:00");
    }

    #[test]
    fn format_recent_line() {
        let r = RecentPlay {
            track_id: 1,
            title: "测试曲".into(),
            artist: Some("歌手".into()),
            source: "qq".into(),
            source_key: "002testmid".into(),
            started_at: 1_786_190_400,
            ended_at: Some(1_786_190_500),
            listened_ms: 95_000,
            reason: "ended".into(),
        };
        let s = format_recent(&r);
        assert!(s.contains("测试曲 - 歌手"));
        assert!(s.contains("listened 95s · ended"));
    }
}
