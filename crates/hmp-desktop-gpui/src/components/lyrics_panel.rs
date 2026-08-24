use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, FontWeight, SharedString, div, prelude::*, px, rgba,
};
use hmp_core::PlaybackState;

use crate::{
    bridge::active_lyric_index,
    state::EventState,
    theme::{ACCENT, layout},
};

#[derive(Default)]
pub struct LyricsPanelState {
    previous_focus: Option<usize>,
    direction: f32,
    animation_generation: u64,
}

impl LyricsPanelState {
    fn update_focus(&mut self, focus: usize) -> (f32, Option<u64>) {
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
    let focus = active.unwrap_or(0);
    let (start, end) = visible_range(events.lyrics.len(), focus, if fullscreen { 11 } else { 9 });
    let rows = events.lyrics[start..end]
        .iter()
        .enumerate()
        .map(|(offset, line)| {
            let index = start + offset;
            let is_active = active == Some(index);
            let distance = index.abs_diff(focus);
            let primary_color = if is_active {
                rgba((ACCENT << 8) | 0xff)
            } else if distance <= 1 {
                rgba(0xf1f1f4a8)
            } else {
                rgba(0xb7b8c25c)
            };
            let translation = line.translation.clone();
            let has_translation = !translation.trim().is_empty();
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(3.))
                .text_center()
                .child(
                    div()
                        .w_full()
                        .text_size(px(if fullscreen {
                            if is_active { 28. } else { 20. }
                        } else if is_active {
                            18.
                        } else {
                            14.
                        }))
                        .font_weight(if is_active {
                            FontWeight::BOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(primary_color)
                        .child(line.text.clone()),
                )
                .when(has_translation, |row| {
                    row.child(
                        div()
                            .w_full()
                            .text_size(px(if fullscreen { 13. } else { 10. }))
                            .text_color(if is_active {
                                rgba(0xf2d7deba)
                            } else {
                                rgba(0xb7b8c254)
                            })
                            .child(translation),
                    )
                })
                .into_any_element()
        })
        .collect::<Vec<_>>();
    let (direction, animation_generation) = state.update_focus(focus);
    let rows = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(if fullscreen { 13. } else { 10. }))
        .children(rows);
    let rows = if let Some(generation) = animation_generation {
        rows.with_animation(
            SharedString::from(format!(
                "hmp-lyrics-{}-{generation}",
                if fullscreen { "fullscreen" } else { "inline" }
            )),
            Animation::new(Duration::from_millis(540)),
            move |rows, delta| {
                let spring = scroll_spring_progress(delta);
                rows.mt(px((1.0 - spring)
                    * if fullscreen { 74.0 } else { 52.0 }
                    * direction))
                    .opacity(0.9 + delta * 0.1)
            },
        )
        .into_any_element()
    } else {
        rows.into_any_element()
    };

    div()
        .relative()
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .when(fullscreen, |panel| panel.flex_1().min_w_0())
        .when(!fullscreen, |panel| {
            panel
                .w(px(layout::LYRICS_WIDTH))
                .bg(rgba(0x12141cff))
                .border_l_1()
                .border_color(rgba(0xffffff10))
        })
        .child(
            div()
                .h(px(if fullscreen { 70. } else { 64. }))
                .px(px(if fullscreen { 34. } else { 24. }))
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(10.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(0xb7b8c25c))
                .child(if fullscreen { "歌词" } else { "LYRICS" })
                .child(if events.lyrics.is_empty() {
                    ""
                } else {
                    "LINE SYNC"
                }),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .px(px(if fullscreen { 44. } else { 22. }))
                .pb(px(if fullscreen { 110. } else { 76. }))
                .flex()
                .flex_col()
                .justify_center()
                .when(events.lyrics_loading, |panel| {
                    panel.child(
                        div()
                            .text_center()
                            .text_size(px(11.))
                            .text_color(rgba(0xb7b8c278))
                            .child("正在加载歌词…"),
                    )
                })
                .when_some(events.lyrics_error.clone(), |panel, message| {
                    panel.child(
                        div()
                            .text_center()
                            .text_size(px(11.))
                            .text_color(rgba(0xffc6cda0))
                            .child(message),
                    )
                })
                .when(
                    !events.lyrics_loading
                        && events.lyrics_error.is_none()
                        && events.lyrics.is_empty(),
                    |panel| {
                        panel.child(
                            div()
                                .text_center()
                                .text_size(px(11.))
                                .text_color(rgba(0xb7b8c278))
                                .child("播放歌曲后将在这里显示同步歌词"),
                        )
                    },
                )
                .when(!events.lyrics.is_empty(), |panel| panel.child(rows)),
        )
        .into_any_element()
}

fn visible_range(length: usize, focus: usize, visible: usize) -> (usize, usize) {
    if length == 0 {
        return (0, 0);
    }
    let visible = visible.min(length);
    let half = visible / 2;
    let mut start = focus.saturating_sub(half);
    let end = (start + visible).min(length);
    start = end.saturating_sub(visible);
    (start, end)
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
        assert_eq!(state.update_focus(2), (0.0, None));

        let (direction, first_generation) = state.update_focus(3);
        assert_eq!(direction, 1.0);
        let first_generation = first_generation.unwrap();

        let (direction, second_generation) = state.update_focus(1);
        assert_eq!(direction, -1.0);
        assert!(second_generation.unwrap() > first_generation);
    }
}
