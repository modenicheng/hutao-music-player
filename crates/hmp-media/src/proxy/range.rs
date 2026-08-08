//! HTTP `Range: bytes=` 请求头解析。
//!
//! 仅支持单区间 `bytes=a-b` / `bytes=a-`；后缀式、多区间、非法格式
//! 一律视为格式错误。

/// 闭区间 `[start, end]`（含两端）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// 区间起始字节（含）。
    pub start: u64,
    /// 区间结束字节（含）。
    pub end: u64,
}

/// Range 头解析失败原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeError {
    /// 格式非法（非 `bytes=` 前缀、后缀式、多区间、非数字等）。
    Malformed,
    /// 区间语义不满足：`start > end` 或 `start >= total`。
    Unsatisfiable,
}

/// 解析 HTTP `Range` 头值（去除 `Range: ` 前缀后的值）。
///
/// - `bytes=a-b` → `ByteRange { start: a, end: b }`
/// - `bytes=a-`  → `ByteRange { start: a, end: total - 1 }`
/// - 后缀式 `bytes=-N`、多区间 `bytes=1-2,3-4`、非法字符 →
///   `Err(RangeError::Malformed)`
/// - `start > end` 或 `start >= total` → `Err(RangeError::Unsatisfiable)`
///
/// # 注意
///
/// 调用方应在确认请求带有 `Range` 头之后再调用本函数；
/// 空字符串或缺少 `bytes=` 前缀一律返回 `Malformed`。
pub fn parse_range(header: &str, total: u64) -> Result<ByteRange, RangeError> {
    // 剥离 "bytes=" 前缀
    let range_spec = header.strip_prefix("bytes=").ok_or(RangeError::Malformed)?;

    // 拒绝多区间（含逗号）
    if range_spec.contains(',') {
        return Err(RangeError::Malformed);
    }

    // 拒绝后缀式（以 '-' 开头）
    if range_spec.starts_with('-') {
        return Err(RangeError::Malformed);
    }

    // 按 '-' 拆分 start / end
    let (start_str, end_str) = range_spec.split_once('-').ok_or(RangeError::Malformed)?;

    // 解析 start（必须为有效非负整数）
    let start: u64 = start_str.parse().map_err(|_| RangeError::Malformed)?;

    if end_str.is_empty() {
        // bytes=a- → end = total - 1
        if start >= total {
            return Err(RangeError::Unsatisfiable);
        }
        Ok(ByteRange {
            start,
            end: total.saturating_sub(1),
        })
    } else {
        // bytes=a-b
        let end: u64 = end_str.parse().map_err(|_| RangeError::Malformed)?;
        if start > end || start >= total {
            return Err(RangeError::Unsatisfiable);
        }
        Ok(ByteRange { start, end })
    }
}

/// 将请求区间裁剪到实际音频长度内。
///
/// - `end >= audio_len` 时截断为 `audio_len - 1`
/// - `start >= audio_len` 时返回 `ByteRange { start, end: start }`，
///   由调用方判定 416
///
/// 该函数用于防御请求的 footer 或 errant 区间超出实际数据。
pub fn clamp_end(start: u64, end: u64, audio_len: u64) -> ByteRange {
    if start >= audio_len {
        ByteRange { start, end: start }
    } else if end >= audio_len {
        ByteRange {
            start,
            end: audio_len.saturating_sub(1),
        }
    } else {
        ByteRange { start, end }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_range 合法输入 ──────────────────────────────────────

    #[test]
    fn parse_range_forms() {
        let total = 300; // 使用大 total 避免 bytes=100- 触发 Unsatisfiable

        // bytes=0- → {0, total-1}
        let r = parse_range("bytes=0-", total).unwrap();
        assert_eq!(r, ByteRange { start: 0, end: 299 });

        // bytes=100-199 → {100, 199}
        let r = parse_range("bytes=100-199", total).unwrap();
        assert_eq!(
            r,
            ByteRange {
                start: 100,
                end: 199
            }
        );

        // bytes=100- → {100, total-1}
        let r = parse_range("bytes=100-", total).unwrap();
        assert_eq!(
            r,
            ByteRange {
                start: 100,
                end: 299
            }
        );
    }

    #[test]
    fn parse_range_malformed() {
        let total = 100;

        // 后缀式
        assert_eq!(parse_range("bytes=-50", total), Err(RangeError::Malformed));

        // 多区间
        assert_eq!(
            parse_range("bytes=1-2,3-4", total),
            Err(RangeError::Malformed)
        );

        // 空字符串
        assert_eq!(parse_range("", total), Err(RangeError::Malformed));

        // 缺少 bytes= 前缀
        assert_eq!(parse_range("100-200", total), Err(RangeError::Malformed));

        // 非数字
        assert_eq!(
            parse_range("bytes=abc-def", total),
            Err(RangeError::Malformed)
        );
    }

    #[test]
    fn parse_range_unsatisfiable() {
        let total = 100;

        // start >= total
        assert_eq!(
            parse_range("bytes=200-300", total),
            Err(RangeError::Unsatisfiable)
        );

        // start > end
        assert_eq!(
            parse_range("bytes=50-20", total),
            Err(RangeError::Unsatisfiable)
        );

        // bytes=100- 且 total=100 → start(100) >= total(100)
        assert_eq!(
            parse_range("bytes=100-", total),
            Err(RangeError::Unsatisfiable)
        );
    }

    // ── clamp_end ─────────────────────────────────────────────────

    #[test]
    fn clamp_end_caps_at_audio_len() {
        let audio_len = 100;

        // end 超大 → 截断至 audio_len-1
        let r = clamp_end(0, 1 << 40, audio_len);
        assert_eq!(r, ByteRange { start: 0, end: 99 });

        // 正常区间不变
        let r = clamp_end(50, 60, audio_len);
        assert_eq!(r, ByteRange { start: 50, end: 60 });

        // start >= audio_len → 返回 {start, start} 供调用方判 416
        let r = clamp_end(100, 120, audio_len);
        assert_eq!(
            r,
            ByteRange {
                start: 100,
                end: 100
            }
        );
    }
}
