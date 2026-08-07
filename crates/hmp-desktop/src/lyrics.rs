use std::collections::HashMap;

use crate::app::UiLyricData;

/// Parse LRC lyric and translation text into timestamped UI rows.
pub fn parse_lrc(lyric: &str, translation: &str) -> Vec<UiLyricData> {
    let translations = parse_lines(translation)
        .into_iter()
        .map(|line| (line.timestamp_ms, line.text))
        .collect::<HashMap<_, _>>();

    let mut lines = parse_lines(lyric)
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

fn parse_lines(source: &str) -> Vec<ParsedLine> {
    let mut parsed = Vec::new();
    for raw_line in source.lines() {
        let mut rest = raw_line.trim();
        let mut timestamps = Vec::new();
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[1..end];
            let Some(timestamp_ms) = parse_timestamp(tag) else {
                break;
            };
            timestamps.push(timestamp_ms);
            rest = rest[end + 1..].trim_start();
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
    parsed
}

fn parse_timestamp(value: &str) -> Option<u64> {
    let (minutes, seconds) = value.split_once(':')?;
    let minutes = minutes.parse::<u64>().ok()?;
    let (seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
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
}
