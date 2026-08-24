use std::time::Duration;

use hmp_core::{LoopMode, PlaybackState, PlaybackStatus};
use hmp_desktop_common::UiLyricData;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatIconState {
    Off,
    All,
    One,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackDisplay {
    pub title: String,
    pub artist: String,
}

pub fn track_display(state: &PlaybackState) -> TrackDisplay {
    state
        .current
        .as_ref()
        .map(|track| TrackDisplay {
            title: track.title.clone(),
            artist: match track.artist_names() {
                artist if !artist.is_empty() => artist,
                _ => "未知歌手".to_owned(),
            },
        })
        .unwrap_or_else(|| TrackDisplay {
            title: "尚未播放".to_owned(),
            artist: "从搜索或媒体库选择歌曲".to_owned(),
        })
}

pub const fn repeat_icon_state(mode: LoopMode) -> RepeatIconState {
    match mode {
        LoopMode::None => RepeatIconState::Off,
        LoopMode::List => RepeatIconState::All,
        LoopMode::Track => RepeatIconState::One,
    }
}

pub const fn next_loop_mode(mode: LoopMode) -> LoopMode {
    match mode {
        LoopMode::None => LoopMode::List,
        LoopMode::List => LoopMode::Track,
        LoopMode::Track => LoopMode::None,
    }
}

pub fn active_lyric_index(lines: &[UiLyricData], position: Duration) -> Option<usize> {
    let position_ms = position.as_millis();
    lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, line)| u128::from(line.timestamp_ms) <= position_ms)
        .map(|(index, _)| index)
}

pub fn is_playing(state: &PlaybackState) -> bool {
    state.status == PlaybackStatus::Playing
}

pub fn progress(state: &PlaybackState) -> f32 {
    let Some(duration) = state.duration.filter(|duration| !duration.is_zero()) else {
        return 0.0;
    };
    (state.position.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0) as f32
}

pub fn elapsed_text(state: &PlaybackState) -> String {
    format_time(state.position)
}

pub fn remaining_text(state: &PlaybackState) -> String {
    state
        .duration
        .map(|duration| format_time(duration.saturating_sub(state.position)))
        .unwrap_or_else(|| "--".into())
}

fn format_time(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    format!("{}:{:02}", total_seconds / 60, total_seconds % 60)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hmp_core::{ArtistId, ArtistRef, LoopMode, PlaybackState, PlaybackStatus, Track, TrackId};
    use hmp_desktop_common::UiLyricData;

    use super::{
        RepeatIconState, active_lyric_index, elapsed_text, is_playing, next_loop_mode, progress,
        remaining_text, repeat_icon_state, track_display,
    };

    #[test]
    fn loop_modes_map_to_stable_icon_states_and_cycle() {
        assert_eq!(repeat_icon_state(LoopMode::None), RepeatIconState::Off);
        assert_eq!(repeat_icon_state(LoopMode::List), RepeatIconState::All);
        assert_eq!(repeat_icon_state(LoopMode::Track), RepeatIconState::One);

        assert_eq!(next_loop_mode(LoopMode::None), LoopMode::List);
        assert_eq!(next_loop_mode(LoopMode::List), LoopMode::Track);
        assert_eq!(next_loop_mode(LoopMode::Track), LoopMode::None);
    }

    #[test]
    fn track_display_handles_empty_track_change_and_buffering_states() {
        let mut state = PlaybackState::default();
        let empty = track_display(&state);
        assert_eq!(empty.title, "尚未播放");

        let mut first = Track::new(TrackId::new("first"), "First Song");
        first.artists.push(ArtistRef {
            id: ArtistId::new("artist"),
            name: "Singer".to_owned(),
        });
        state.current = Some(first);
        state.status = PlaybackStatus::Playing;
        assert_eq!(track_display(&state).artist, "Singer");

        state.current = Some(Track::new(TrackId::new("second"), "Second Song"));
        state.status = PlaybackStatus::Buffering;
        state.buffering = Some(0.5);
        let changed = track_display(&state);
        assert_eq!(changed.title, "Second Song");
        assert_eq!(changed.artist, "未知歌手");

        state.status = PlaybackStatus::Paused;
        state.duration = None;
        assert_eq!(track_display(&state), changed);
    }

    #[test]
    fn progress_is_zero_without_a_positive_duration() {
        let mut state = PlaybackState::default();
        state.position = Duration::from_secs(10);
        assert_eq!(progress(&state), 0.0);
        assert_eq!(remaining_text(&state), "--");

        state.duration = Some(Duration::ZERO);
        assert_eq!(progress(&state), 0.0);
    }

    #[test]
    fn progress_and_time_text_are_clamped_to_the_duration() {
        let mut state = PlaybackState::default();
        state.position = Duration::from_secs(50);
        state.duration = Some(Duration::from_secs(200));
        assert_eq!(progress(&state), 0.25);
        assert_eq!(elapsed_text(&state), "0:50");
        assert_eq!(remaining_text(&state), "2:30");

        state.position = Duration::from_secs(250);
        assert_eq!(progress(&state), 1.0);
        assert_eq!(remaining_text(&state), "0:00");
    }

    #[test]
    fn only_authoritative_playing_status_renders_as_playing() {
        let mut state = PlaybackState::default();
        for status in [
            PlaybackStatus::Empty,
            PlaybackStatus::Loading,
            PlaybackStatus::Buffering,
            PlaybackStatus::Paused,
            PlaybackStatus::Stopped,
            PlaybackStatus::Ended,
            PlaybackStatus::Error,
        ] {
            state.status = status;
            assert!(!is_playing(&state));
        }
        state.status = PlaybackStatus::Playing;
        assert!(is_playing(&state));
    }

    #[test]
    fn active_lyric_is_the_latest_line_not_after_playback_position() {
        let lines = [
            lyric(1_000, "one"),
            lyric(2_500, "two"),
            lyric(5_000, "three"),
        ];

        assert_eq!(active_lyric_index(&lines, Duration::from_millis(999)), None);
        assert_eq!(
            active_lyric_index(&lines, Duration::from_millis(1_000)),
            Some(0)
        );
        assert_eq!(
            active_lyric_index(&lines, Duration::from_millis(4_999)),
            Some(1)
        );
        assert_eq!(
            active_lyric_index(&lines, Duration::from_millis(9_000)),
            Some(2)
        );
    }

    fn lyric(timestamp_ms: u64, text: &str) -> UiLyricData {
        UiLyricData {
            timestamp_ms,
            time: String::new(),
            text: text.into(),
            translation: String::new(),
        }
    }
}
