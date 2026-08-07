use std::collections::HashMap;

use crate::app::UiLyricData;

/// Parse LRC lyric and translation text into timestamped UI rows.
pub fn parse_lrc(lyric: &str, translation: &str) -> Vec<UiLyricData> {
    let (_, translation_lines) = parse_source(translation);
    let translations = translation_lines
        .into_iter()
        .map(|line| (line.timestamp_ms, line.text))
        .collect::<HashMap<_, _>>();

    let (_, lyric_lines) = parse_source(lyric);
    let mut lines = lyric_lines
        .into_iter()
        .map(|line| UiLyricData {
            timestamp_ms: line.timestamp_ms,
            time: format_time(line.timestamp_ms),
            text: line.text,
            translation: translations
                .get(&line.timestamp_ms)
                .cloned()
                .unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    lines.sort_by_key(|line| line.timestamp_ms);
    lines
}

#[derive(Debug)]
struct ParsedLine {
    timestamp_ms: u64,
    text: String,
}

fn parse_source(source: &str) -> (i64, Vec<ParsedLine>) {
    let mut offset_ms = 0i64;
    let mut parsed = Vec::new();
    for raw_line in source.lines() {
        let mut rest = raw_line.trim();
        let mut timestamps = Vec::new();
        loop {
            let Some(end) = rest.strip_prefix('[').and_then(|value| value.find(']')) else {
                break;
            };
            let tag_end = end + 1;
            let tag = &rest[1..tag_end];
            if let Some(offset) = tag
                .strip_prefix("offset:")
                .and_then(|value| value.parse::<i64>().ok())
            {
                offset_ms = offset;
                rest = rest[tag_end + 1..].trim_start();
                continue;
            }
            let Some(timestamp_ms) = parse_timestamp(tag) else {
                break;
            };
            timestamps.push(timestamp_ms);
            rest = rest[tag_end + 1..].trim_start();
        }
        let text = rest.trim();
        if text.is_empty() {
            continue;
        }
        for timestamp_ms in timestamps {
            parsed.push(ParsedLine {
                timestamp_ms,
                text: text.to_owned(),
            });
        }
    }
    for line in &mut parsed {
        line.timestamp_ms = apply_offset(line.timestamp_ms, offset_ms);
    }
    (offset_ms, parsed)
}

fn apply_offset(timestamp_ms: u64, offset_ms: i64) -> u64 {
    if offset_ms >= 0 {
        timestamp_ms.saturating_add(offset_ms as u64)
    } else {
        timestamp_ms.saturating_sub(offset_ms.unsigned_abs())
    }
}

fn parse_timestamp(value: &str) -> Option<u64> {
    let (minutes, seconds) = value.split_once(':')?;
    let minutes = minutes.parse::<u64>().ok()?;
    let (seconds, fraction) = seconds
        .split_once(|separator| separator == '.' || separator == ',')
        .unwrap_or((seconds, ""));
    let seconds = seconds.parse::<u64>().ok()?;
    if seconds >= 60 {
        return None;
    }
    let millis = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u64>().ok()?.saturating_mul(100),
        2 => fraction.parse::<u64>().ok()?.saturating_mul(10),
        3 => fraction.parse::<u64>().ok()?,
        _ => return None,
    };
    Some(minutes.saturating_mul(60_000) + seconds.saturating_mul(1_000) + millis)
}

fn format_time(timestamp_ms: u64) -> String {
    let total_seconds = timestamp_ms / 1_000;
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc_lines_and_matches_translation_by_timestamp() {
        let lines = parse_lrc(
            "[ti:Test]\n[00:01.20]First\n[00:03.00]Second\n",
            "[00:01.20]译文\n",
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].timestamp_ms, 1_200);
        assert_eq!(lines[0].text, "First");
        assert_eq!(lines[0].translation, "译文");
    }

    #[test]
    fn ignores_lrc_metadata_and_malformed_lines() {
        let lines = parse_lrc("[ar:Artist]\nnot-a-line\n[00:02]Valid\n", "");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].time, "00:02");
    }

    #[test]
    fn parses_comma_fractions_and_applies_lrc_offset() {
        let lines = parse_lrc(
            "[offset:-1500]\n[00:01,50]Before\n[00:02.500]After\n",
            "[offset:-500]\n[00:00,50]译文\n[00:01.500]另一个译文\n",
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].timestamp_ms, 0);
        assert_eq!(lines[0].text, "Before");
        assert_eq!(lines[0].translation, "译文");
        assert_eq!(lines[1].timestamp_ms, 1_000);
        assert_eq!(lines[1].translation, "另一个译文");
    }
}
