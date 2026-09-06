//! 展示层格式化助手（移植自 apps/hmp-tauri/src/lib/format.ts 及
//! LibraryView 的 formatCount）。纯函数、确定性输出。

/// 字节数 → 人读大小：≥1 GB 以 GB 计，否则以 MB 计；一位小数，整值去尾 ".0"
pub fn format_bytes(bytes: u64) -> String {
    let gb = bytes as f64 / 1024f64.powi(3);
    if gb >= 1.0 {
        format!("{:.1$} GB", gb, decimals(gb))
    } else {
        let mb = bytes as f64 / 1024f64.powi(2);
        format!("{:.1$} MB", mb, decimals(mb))
    }
}

/// 与 TS 版 trimOne 对齐：一位小数，".0" 去尾
fn decimals(value: f64) -> usize {
    let fixed = format!("{value:.1}");
    if fixed.ends_with(".0") {
        0
    } else {
        1
    }
}

/// 毫秒 → 长时长：一小时内 "46 分钟"，跨小时 "3 小时 42 分钟"（分钟数 round，对齐 TS）
pub fn format_long_duration(ms: u64) -> String {
    let minutes = (ms as f64 / 60_000.0).round() as i64;
    let hours = minutes / 60;
    let rest = minutes % 60;
    if hours > 0 {
        format!("{hours} 小时 {rest} 分钟")
    } else {
        format!("{rest} 分钟")
    }
}

/// 分 → 金额文案："¥36.00"（恒两位小数）
pub fn format_cny(fen: u64) -> String {
    format!("¥{:.2}", fen as f64 / 100.0)
}

/// 播放次数 → "128.5万" / "8621"（LibraryView.formatCount 同文案）
pub fn format_count_wan(value: u64) -> String {
    if value >= 10_000 {
        let v = value as f64 / 10_000.0;
        format!("{:.1$}万", v, decimals(v))
    } else {
        value.to_string()
    }
}

/// 毫秒 → "m:ss"（TrackTable 时长列，秒数 round 对齐 TS）
pub fn format_duration(ms: u64) -> String {
    let total_seconds = (ms as f64 / 1000.0).round() as u64;
    format!("{}:{:02}", total_seconds / 60, total_seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_matches_ts() {
        // TS trimOne(0): "0.0" → 去尾 ".0" → "0"
        assert_eq!(format_bytes(0), "0 MB");
        assert_eq!(format_bytes(1024 * 1024), "1 MB");
        assert_eq!(format_bytes(1536 * 1024 * 1024), "1.5 GB");
        assert_eq!(format_bytes(850 * 266_000 / 8), "27 MB");
    }

    #[test]
    fn long_duration_matches_ts() {
        assert_eq!(format_long_duration(46 * 60_000), "46 分钟");
        assert_eq!(format_long_duration(222 * 60_000), "3 小时 42 分钟");
    }

    #[test]
    fn cny_matches_ts() {
        assert_eq!(format_cny(3600), "¥36.00");
        assert_eq!(format_cny(205), "¥2.05");
    }

    #[test]
    fn count_wan() {
        assert_eq!(format_count_wan(1_284_567), "128.5万");
        assert_eq!(format_count_wan(8621), "8621");
    }

    #[test]
    fn duration() {
        assert_eq!(format_duration(252_000), "4:12");
        // TS Math.round：61.4s → 61s
        assert_eq!(format_duration(61_400), "1:01");
    }
}
