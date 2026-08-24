#![allow(dead_code)]

pub const SIDEBAR_WIDTH: f32 = 230.0;
pub const SIDEBAR_GUTTER: f32 = 7.0;
pub const LYRICS_WIDTH: f32 = 360.0;
pub const HIDE_LYRICS_BELOW: f32 = 1080.0;
pub const COMPACT_TOP_BAR_BELOW: f32 = 980.0;
pub const TOP_BAR_HEIGHT: f32 = 56.0;
pub const FLOATING_PLAYER_BOTTOM: f32 = 14.0;
pub const FLOATING_PLAYER_RADIUS: f32 = 24.0;
pub const TOP_BAR_SEARCH_WIDTH: f32 = 412.0;
pub const TOP_BAR_SEARCH_WIDTH_COMPACT: f32 = 310.0;
pub const FLOATING_PLAYER_WIDTH: f32 = 600.0;
pub const FLOATING_PLAYER_WIDTH_COMPACT: f32 = 500.0;

pub fn top_bar_search_width(window_width: f32) -> f32 {
    if window_width < COMPACT_TOP_BAR_BELOW {
        TOP_BAR_SEARCH_WIDTH_COMPACT
    } else {
        TOP_BAR_SEARCH_WIDTH
    }
}

pub fn floating_player_width(window_width: f32) -> f32 {
    if window_width < COMPACT_TOP_BAR_BELOW {
        FLOATING_PLAYER_WIDTH_COMPACT
    } else {
        FLOATING_PLAYER_WIDTH
    }
}

pub fn now_playing_cover_size(window_width: f32, window_height: f32) -> f32 {
    (window_height - 310.0)
        .min(window_width * 0.30)
        .clamp(280.0, 430.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_shell_geometry_matches_upstream_demo() {
        assert_eq!(SIDEBAR_WIDTH, 230.0);
        assert_eq!(SIDEBAR_GUTTER, 7.0);
        assert_eq!(LYRICS_WIDTH, 360.0);
        assert_eq!(TOP_BAR_HEIGHT, 56.0);
        assert_eq!(FLOATING_PLAYER_BOTTOM, 14.0);
        assert_eq!(FLOATING_PLAYER_RADIUS, 24.0);
    }

    #[test]
    fn reference_responsive_breakpoints_are_stable() {
        assert_eq!(HIDE_LYRICS_BELOW, 1080.0);
        assert_eq!(COMPACT_TOP_BAR_BELOW, 980.0);
        assert_eq!(top_bar_search_width(1280.0), 412.0);
        assert_eq!(top_bar_search_width(900.0), 310.0);
        assert_eq!(floating_player_width(1280.0), 600.0);
        assert_eq!(floating_player_width(900.0), 500.0);
    }

    #[test]
    fn reference_now_playing_cover_scale_is_bounded() {
        assert_eq!(now_playing_cover_size(1280.0, 800.0), 384.0);
        assert_eq!(now_playing_cover_size(900.0, 700.0), 280.0);
        assert_eq!(now_playing_cover_size(1920.0, 1200.0), 430.0);
    }
}
