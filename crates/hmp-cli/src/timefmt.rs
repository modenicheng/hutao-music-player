//! 无 chrono 依赖的 unix 时间戳格式化（Howard Hinnant 民用历法算法）。
//!
//! `history`（UTC）与 `comment`（QQ 评论时间，固定 UTC+8）共用。

/// unix 秒 → `YYYY-MM-DD HH:MM`（指定 UTC 偏移，秒）。
pub fn format_with_offset(ts: i64, utc_offset_secs: i64) -> Option<String> {
    let secs = ts + utc_offset_secs;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = civil_from_days(days)?;
    Some(format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}"))
}

/// unix 秒 → `YYYY-MM-DD HH:MM`（UTC）。
pub fn format_utc(ts: i64) -> String {
    format_with_offset(ts, 0).unwrap_or_else(|| ts.to_string())
}

/// 天数（自 1970-01-01）→ (年, 月, 日)：Howard Hinnant `civil_from_days`。
fn civil_from_days(z: i64) -> Option<(i64, i64, i64)> {
    let z = z.checked_add(719_468)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_leap() {
        assert_eq!(format_utc(0), "1970-01-01 00:00");
        assert_eq!(civil_from_days(0), Some((1970, 1, 1)));
        assert_eq!(civil_from_days(20_670), Some((2026, 8, 5)));
        assert_eq!(civil_from_days(20_674), Some((2026, 8, 9)));
        // 闰年 2000-02-29（天 11016）
        assert_eq!(civil_from_days(11_016), Some((2000, 2, 29)));
    }

    #[test]
    fn utc8_offset() {
        // 2026-08-09 00:00 UTC = 08:00 UTC+8
        let s = format_with_offset(1_786_233_600, 8 * 3600);
        assert_eq!(s.as_deref(), Some("2026-08-09 08:00"));
    }
}
