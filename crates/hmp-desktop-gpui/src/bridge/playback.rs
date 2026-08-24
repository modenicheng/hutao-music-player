use std::time::Duration;

use hmp_core::{PlaybackState, PlaybackStatus};

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
        .unwrap_or_else(|| "--:--".into())
}

fn format_time(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hmp_core::{PlaybackState, PlaybackStatus};

    use super::{elapsed_text, is_playing, progress, remaining_text};

    #[test]
    fn progress_is_zero_without_a_positive_duration() {
        let mut state = PlaybackState::default();
        state.position = Duration::from_secs(10);
        assert_eq!(progress(&state), 0.0);
        assert_eq!(remaining_text(&state), "--:--");

        state.duration = Some(Duration::ZERO);
        assert_eq!(progress(&state), 0.0);
    }

    #[test]
    fn progress_and_time_text_are_clamped_to_the_duration() {
        let mut state = PlaybackState::default();
        state.position = Duration::from_secs(50);
        state.duration = Some(Duration::from_secs(200));
        assert_eq!(progress(&state), 0.25);
        assert_eq!(elapsed_text(&state), "00:50");
        assert_eq!(remaining_text(&state), "02:30");

        state.position = Duration::from_secs(250);
        assert_eq!(progress(&state), 1.0);
        assert_eq!(remaining_text(&state), "00:00");
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
}
