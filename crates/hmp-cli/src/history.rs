//! `hmp history`：最近播放（直读媒体库；daemon 与 CLI 同机同文件，WAL 并发安全）。

use std::io::Write;

use hmp_storage::{LibraryDb, RecentPlay};

/// 格式化一条最近播放。
pub fn format_recent(r: &RecentPlay) -> String {
    let artist = r.artist.as_deref().unwrap_or("");
    let listened = r.listened_ms / 1000;
    let status = if r.ended_at.is_some() {
        format!("(听过 {listened}s · {})", r.reason)
    } else {
        "（播放中）".to_string()
    };
    format!(
        "{:>2}. {} - {artist}  {status}  {}",
        r.track_id,
        r.title,
        fmt_time(r.started_at)
    )
}

/// 打印最近播放列表（默认 10 条）。
pub async fn run(limit: Option<u32>) -> Result<(), Box<dyn std::error::Error>> {
    let path = hmp_storage::data_dir().join("library.sqlite3");
    if !path.exists() {
        eprintln!("媒体库不存在（尚无播放记录）：{}", path.display());
        return Ok(());
    }
    let mut db = LibraryDb::open(&path)?;
    let plays = db.recent_plays(limit.unwrap_or(10))?;
    let mut stdout = std::io::stdout().lock();
    if plays.is_empty() {
        writeln!(stdout, "暂无播放记录")?;
    } else {
        for r in &plays {
            writeln!(stdout, "{}", format_recent(r))?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// unix 秒 → `YYYY-MM-DD HH:MM`（无 chrono 依赖，Howard Hinnant 算法）。
fn fmt_time(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let rem = ts.rem_euclid(86_400);
    let (h, m) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
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
            started_at: 1_786_190_400,
            ended_at: Some(1_786_190_500),
            listened_ms: 95_000,
            reason: "ended".into(),
        };
        let s = format_recent(&r);
        assert!(s.contains("测试曲 - 歌手"));
        assert!(s.contains("听过 95s · ended"));
    }
}
