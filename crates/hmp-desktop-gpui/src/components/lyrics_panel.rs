//! Reference lyrics hierarchy adapted to HMP line-synced lyric events.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, FontWeight, SharedString, div, linear_color_stop,
    linear_gradient, prelude::*, px, relative, rgb, rgba,
};
use hmp_core::PlaybackState;

use crate::{bridge::active_lyric_index, state::EventState, theme::layout};

#[derive(Default)]
pub struct LyricsPanelState {
    inline: LyricsScrollState,
    fullscreen: LyricsScrollState,
}

#[derive(Default)]
struct LyricsScrollState {
    previous_focus: Option<usize>,
    direction: f32,
    animation_generation: u64,
}

impl LyricsScrollState {
    fn update(&mut self, focus: usize) -> (f32, Option<u64>) {
        let Some(previous) = self.previous_focus.replace(focus) else {
            return (0.0, None);
        };
        if previous == focus {
            return (self.direction, None);
        }
        self.direction = if focus > previous { 1.0 } else { -1.0 };
        self.animation_generation = self.animation_generation.wrapping_add(1);
        (self.direction, Some(self.animation_generation))
    }
}

impl LyricsPanelState {
    fn update_inline_focus(&mut self, focus: usize) -> (f32, Option<u64>) {
        self.inline.update(focus)
    }

    fn update_fullscreen_focus(&mut self, focus: usize) -> (f32, Option<u64>) {
        self.fullscreen.update(focus)
    }
}

pub fn render_inline(
    playback: &PlaybackState,
    events: &EventState,
    state: &mut LyricsPanelState,
) -> AnyElement {
    render_panel(playback, events, state, false)
}

pub fn render_fullscreen(
    playback: &PlaybackState,
    events: &EventState,
    state: &mut LyricsPanelState,
) -> AnyElement {
    render_panel(playback, events, state, true)
}

fn render_panel(
    playback: &PlaybackState,
    events: &EventState,
    state: &mut LyricsPanelState,
    fullscreen: bool,
) -> AnyElement {
    let active = active_lyric_index(&events.lyrics, playback.position);
    let focus = active.unwrap_or_else(|| {
        events
            .lyrics
            .partition_point(|line| line.timestamp_ms <= playback.position.as_millis() as u64)
            .min(events.lyrics.len().saturating_sub(1))
    });
    let (start, end) = visible_range(events.lyrics.len(), focus, if fullscreen { 9 } else { 11 });
    let rows = events.lyrics[start..end]
        .iter()
        .enumerate()
        .map(|(offset, line)| {
            let index = start + offset;
            lyric_row(line, index, focus, active, fullscreen)
        })
        .collect::<Vec<_>>();
    let (scroll_direction, scroll_animation) = if fullscreen {
        state.update_fullscreen_focus(focus)
    } else {
        state.update_inline_focus(focus)
    };
    let scroll_distance = if fullscreen { 74.0 } else { 52.0 };
    let rows = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(if fullscreen { 10.0 } else { 8.0 }))
        .children(rows);
    let rows = if let Some(generation) = scroll_animation {
        rows.with_animation(
            SharedString::from(format!(
                "lyrics-scroll-{}-{generation}",
                if fullscreen { "fullscreen" } else { "inline" }
            )),
            Animation::new(Duration::from_millis(820)),
            move |rows, delta| {
                let spring = scroll_spring_progress(delta);
                rows.mt(px((1.0 - spring) * scroll_distance * scroll_direction))
                    .opacity(0.92 + smootherstep(delta) * 0.08)
            },
        )
        .into_any_element()
    } else {
        rows.into_any_element()
    };

    div()
        .relative()
        .h_full()
        .when(cfg!(target_os = "linux"), |panel| {
            panel.font_family(crate::theme::FONT_FAMILY)
        })
        .flex_none()
        .flex()
        .flex_col()
        .when(fullscreen, |panel| panel.w_full())
        .when(!fullscreen, |panel| {
            panel
                .w(px(layout::LYRICS_WIDTH))
                .bg(rgb(0x12141c))
                .border_l_1()
                .border_color(rgba(0xffffff10))
        })
        .when(!fullscreen, |panel| {
            panel.child(
                div()
                    .absolute()
                    .top(px(28.))
                    .left(px(28.))
                    .right(px(28.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(9.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgba(0xb7b8c25c))
                    .child("LYRICS")
                    .child(if events.lyrics.is_empty() {
                        ""
                    } else {
                        "LINE SYNC"
                    }),
            )
        })
        .child(
            div()
                .size_full()
                .px(px(if fullscreen { 30.0 } else { 22.0 }))
                .flex()
                .flex_col()
                .justify_center()
                .when(events.lyrics_loading, |panel| {
                    panel.child(empty_copy("正在加载歌词…"))
                })
                .when_some(events.lyrics_error.clone(), |panel, message| {
                    panel.child(empty_copy(message))
                })
                .when(
                    !events.lyrics_loading
                        && events.lyrics_error.is_none()
                        && events.lyrics.is_empty(),
                    |panel| panel.child(empty_copy("播放歌曲后将在这里显示同步歌词")),
                )
                .when(!events.lyrics.is_empty(), |panel| panel.child(rows)),
        )
        .when(!fullscreen, |panel| {
            panel
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(105.))
                        .bg(linear_gradient(
                            180.0,
                            linear_color_stop(rgba(0x12141cff), 0.0),
                            linear_color_stop(rgba(0x12141c00), 1.0),
                        )),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(92.))
                        .bg(linear_gradient(
                            0.0,
                            linear_color_stop(rgba(0x12141cff), 0.0),
                            linear_color_stop(rgba(0x12141c00), 1.0),
                        )),
                )
        })
        .into_any_element()
}

fn empty_copy(copy: impl IntoElement) -> AnyElement {
    div()
        .px(px(24.))
        .text_center()
        .text_size(px(10.))
        .text_color(rgba(0xb7b8c278))
        .child(copy)
        .into_any_element()
}

fn lyric_row(
    line: &hmp_desktop_common::UiLyricData,
    index: usize,
    focus: usize,
    active: Option<usize>,
    large: bool,
) -> AnyElement {
    let distance = index.abs_diff(focus);
    let is_active = active == Some(index);
    let text_alpha: u32 = if active.is_none() {
        match distance {
            0 => 0xc0,
            1 => 0x9c,
            2 => 0x78,
            3 => 0x5c,
            _ => 0x44,
        }
    } else {
        match distance {
            0 => 0xff,
            1 => 0xaa,
            2 => 0x82,
            3 => 0x65,
            _ => 0x48,
        }
    };
    let resting_text_size = if large { 24.0 } else { 16.0 };
    let active_text_size = if large { 26.0 } else { 17.0 };
    let text_size = if is_active {
        active_text_size
    } else {
        resting_text_size
    };
    let translation = line.translation.clone();
    let has_translation = !translation.trim().is_empty();
    let row = div()
        .relative()
        .w_full()
        .min_h(px(if large { 64.0 } else { 44.0 }))
        .px(px(if large { 26.0 } else { 18.0 }))
        .py(px(if large { 3.0 } else { 2.0 }))
        .flex()
        .items_center()
        .rounded(px(18.))
        .text_size(px(text_size))
        .line_height(relative(1.3))
        .text_color(rgba(0xffffff00 | text_alpha))
        .font_weight(FontWeight::BOLD)
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(line.text.clone())
                .when(has_translation, |text| {
                    text.child(
                        div()
                            .mt(px(if large { 5.0 } else { 3.0 }))
                            .text_size(px(if large { 13.5 } else { 10.5 }))
                            .line_height(relative(1.25))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(if is_active {
                                0xffffffb8
                            } else {
                                0xffffff00 | text_alpha.saturating_sub(0x24)
                            }))
                            .child(translation),
                    )
                }),
        );

    if is_active {
        row.with_animation(
            SharedString::from(format!("active-lyric-{index}")),
            Animation::new(Duration::from_millis(440)),
            move |row, delta| {
                row.text_size(px(
                    resting_text_size + (active_text_size - resting_text_size) * delta
                ))
            },
        )
        .into_any_element()
    } else {
        row.into_any_element()
    }
}

fn visible_range(length: usize, focus: usize, max_rows: usize) -> (usize, usize) {
    if length <= max_rows {
        return (0, length);
    }
    let start = focus.saturating_sub(max_rows / 2).min(length - max_rows);
    (start, start + max_rows)
}

fn smootherstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}

fn scroll_spring_progress(delta: f32) -> f32 {
    let delta = delta.clamp(0.0, 1.0);
    if delta >= 1.0 {
        return 1.0;
    }
    1.0 - (-8.0 * delta).exp() * (11.0 * delta).cos()
}

#[cfg(test)]
mod tests {
    use super::LyricsPanelState;

    #[test]
    fn focus_changes_track_scroll_direction_and_animation_generation() {
        let mut state = LyricsPanelState::default();
        assert_eq!(state.update_inline_focus(2), (0.0, None));

        let (direction, first_generation) = state.update_inline_focus(3);
        assert_eq!(direction, 1.0);
        let first_generation = first_generation.unwrap();

        let (direction, second_generation) = state.update_inline_focus(1);
        assert_eq!(direction, -1.0);
        assert!(second_generation.unwrap() > first_generation);
    }

    #[test]
    fn inline_and_fullscreen_focus_animations_are_independent() {
        let mut state = LyricsPanelState::default();
        assert_eq!(state.update_inline_focus(2), (0.0, None));
        assert_eq!(state.update_fullscreen_focus(7), (0.0, None));
        assert_eq!(state.update_inline_focus(3).0, 1.0);
        assert_eq!(state.update_fullscreen_focus(6).0, -1.0);
    }
}
