//! Reference player surfaces adapted from `cradiy/gpui-apple-music-demo` (MIT).
//! All playback actions are routed through HMP `AppCommand`.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::{
    AnyElement, Bounds, Div, FontWeight, MouseButton, MouseDownEvent, ObjectFit, Pixels, Stateful,
    canvas, div, img, prelude::*, px, relative, rgb, rgba, svg,
};
use uic::assets::LucideIcons;

use crate::{
    app::HmpGpuiApp,
    bridge::{
        RepeatIconState, elapsed_text, is_playing, next_loop_mode, progress, remaining_text,
        repeat_icon_state, track_display,
    },
    state::Page,
    theme::{ACCENT, layout},
};

fn small_icon_button(id: &'static str, icon: LucideIcons, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(27.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xffffff0f)))
        .child(svg().path(icon).size(px(14.)).text_color(if enabled {
            rgba(0xd5d5ddeb)
        } else {
            rgba(0xa8a9b34d)
        }))
}

fn icon_button(id: &'static str, icon: LucideIcons, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(27.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xffffff0f)))
        .child(svg().path(icon).size_5().text_color(if enabled {
            rgba(0xd5d5ddeb)
        } else {
            rgba(0xa8a9b34d)
        }))
}

pub fn render(
    app: &mut HmpGpuiApp,
    cx: &mut gpui::Context<HmpGpuiApp>,
    compact: bool,
) -> impl IntoElement {
    let playing = is_playing(&app.playback);
    let progress = progress(&app.playback);
    let elapsed = elapsed_text(&app.playback);
    let remaining = remaining_text(&app.playback);
    let display = track_display(&app.playback);
    let title = display.title;
    let artist = display.artist;
    let artwork = app
        .playback
        .current
        .as_ref()
        .and_then(|track| track.cover.as_ref())
        .map(|cover| cover.url.clone());
    let has_queue = !app.events.queue.is_empty();
    let shuffle = app.playback.shuffle;
    let repeat = repeat_icon_state(app.playback.loop_mode);
    let muted = app.playback.volume <= f64::EPSILON;
    let progress_bounds = Rc::new(Cell::<Option<Bounds<Pixels>>>::new(None));
    let progress_bounds_for_paint = Rc::clone(&progress_bounds);
    let progress_bounds_for_click = Rc::clone(&progress_bounds);

    div()
        .relative()
        .w(px(if compact {
            layout::FLOATING_PLAYER_WIDTH_COMPACT
        } else {
            layout::FLOATING_PLAYER_WIDTH
        }))
        .px(px(10.))
        .m_10()
        .flex()
        .items_center()
        .justify_between()
        .rounded(px(layout::FLOATING_PLAYER_RADIUS))
        .bg(rgba(0x181a23c4))
        .backdrop_blur(px(22.))
        .border_1()
        .border_color(rgba(0xffffff0e))
        .shadow(vec![
            gpui::BoxShadow::new(px(0.), px(9.), rgba(0x00000051).into())
                .blur_radius(px(22.))
                .spread_radius(px(-7.)),
        ])
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    small_icon_button("shuffle", LucideIcons::Shuffle, shuffle).on_click(
                        cx.listener(|app, _, _, _| {
                            app.commands.set_shuffle(!app.playback.shuffle);
                        }),
                    ),
                )
                .child(
                    icon_button("skip-back", LucideIcons::SkipBack, has_queue)
                        .on_click(cx.listener(|app, _, _, _| app.commands.previous())),
                )
                .child(
                    div()
                        .id("play-pause")
                        .size(px(31.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(0xffffff13)))
                        .on_click(cx.listener(|app, _, _, _| app.commands.toggle_play()))
                        .child(
                            svg()
                                .path(if playing {
                                    LucideIcons::Pause
                                } else {
                                    LucideIcons::Play
                                })
                                .size_6()
                                .text_color(if playing {
                                    rgba(0xffffffff)
                                } else {
                                    rgba(0xb9bac35d)
                                }),
                        ),
                )
                .child(
                    icon_button("skip-forward", LucideIcons::SkipForward, has_queue)
                        .on_click(cx.listener(|app, _, _, _| app.commands.next())),
                )
                .child(
                    small_icon_button(
                        "repeat",
                        if repeat == RepeatIconState::One {
                            LucideIcons::Repeat1
                        } else {
                            LucideIcons::Repeat
                        },
                        repeat != RepeatIconState::Off,
                    )
                    .on_click(cx.listener(|app, _, _, _| {
                        app.commands
                            .set_loop_mode(next_loop_mode(app.playback.loop_mode));
                    })),
                ),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .m_2()
                        .items_center()
                        .justify_start()
                        .rounded(px(12.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(11.))
                                .group_hover("player-progress-hover", |style| style.opacity(0.15))
                                .when_some(artwork, |metadata, artwork| {
                                    metadata.child(
                                        div()
                                            .m_2()
                                            .id("open-now-playing")
                                            .relative()
                                            .size(px(32.))
                                            .flex_none()
                                            .overflow_hidden()
                                            .rounded(px(9.))
                                            .group("player-cover-hover")
                                            .cursor_pointer()
                                            .border_1()
                                            .border_color(rgba(0xffffff18))
                                            .shadow(vec![
                                                gpui::BoxShadow::new(
                                                    px(0.),
                                                    px(4.),
                                                    rgba(0x00000048).into(),
                                                )
                                                .blur_radius(px(10.))
                                                .spread_radius(px(-3.)),
                                            ])
                                            .child(
                                                img(artwork)
                                                    .size_full()
                                                    .rounded(px(9.))
                                                    .object_fit(ObjectFit::Cover),
                                            )
                                            .child(
                                                div()
                                                    .absolute()
                                                    .inset_0()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(9.))
                                                    .opacity(0.)
                                                    .group_hover("player-cover-hover", |style| {
                                                        style.opacity(1.).bg(rgba(0x090a0fc2))
                                                    })
                                                    .child(
                                                        svg()
                                                            .path(LucideIcons::Maximize2)
                                                            .size(px(18.))
                                                            .text_color(rgba(0xffffffff)),
                                                    ),
                                            )
                                            .on_click(cx.listener(|app, _, _, cx| {
                                                app.now_playing = true;
                                                app.effects_viewer = None;
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(
                                    div()
                                        .max_w(px(if compact { 125. } else { 230. }))
                                        .flex()
                                        .flex_col()
                                        .items_start()
                                        .child(
                                            div()
                                                .w_full()
                                                .truncate()
                                                .text_size(px(12.))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgba(0xf1f1f4e8))
                                                .child(title),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .mt(px(2.))
                                                .truncate()
                                                .font_weight(FontWeight::BOLD)
                                                .text_size(px(9.5))
                                                .text_color(rgba(0xb7b8c288))
                                                .child(artist),
                                        ),
                                ),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .h_4()
                        .w_full()
                        .flex()
                        .group("player-progress-hover-zone")
                        .bottom_0()
                        .left_0()
                        .items_end()
                        .justify_center()
                        .right_0()
                        .text_color(gpui::white())
                        .child(
                            div()
                                .id("player-progress")
                                .w_full()
                                .absolute()
                                .bottom_0()
                                .flex()
                                .flex_col()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |app, event: &MouseDownEvent, _, _| {
                                        let (Some(duration), Some(bounds)) = (
                                            app.playback.duration,
                                            progress_bounds_for_click.get(),
                                        ) else {
                                            return;
                                        };
                                        if app.playback.can_seek {
                                            app.commands.seek(seek_seconds_for_click(
                                                duration,
                                                event.position.x.as_f32(),
                                                bounds.origin.x.as_f32(),
                                                bounds.size.width.as_f32(),
                                            ));
                                        }
                                    }),
                                )
                                .child(
                                    canvas(
                                        move |bounds, _, _| {
                                            progress_bounds_for_paint.set(Some(bounds));
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                                .child(
                                    div()
                                        .h(px(4.))
                                        .flex()
                                        .absolute()
                                        .bottom(px(6.))
                                        .items_center()
                                        .w_full()
                                        .opacity(0.)
                                        .group_hover("player-progress-hover-zone", |style| {
                                            style
                                                .opacity(1.)
                                                .bg(rgba(0x11131aad))
                                                .backdrop_blur(px(18.))
                                        })
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .mr(px(4.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgba(0xffffffe5))
                                                .child(elapsed),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .flex()
                                                .justify_start()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .bg(rgba(0xffffffff))
                                                        .w(relative(progress))
                                                        .h(px(4.))
                                                        .rounded_full(),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .ml(px(4.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgba(0xffffffe5))
                                                .child(remaining),
                                        ),
                                )
                                .child(
                                    div()
                                        .h(px(1.))
                                        .absolute()
                                        .bottom(px(6.))
                                        .ml_4()
                                        .mr_4()
                                        .bg(gpui::white())
                                        .group_hover("player-progress-hover-zone", |this| {
                                            this.opacity(0.0)
                                        })
                                        .w(relative(progress)),
                                ),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_4()
                .child(
                    div()
                        .id("lyrics")
                        .size(px(27.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(0xffffff0f)))
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.navigation.navigate(Page::Lyrics);
                            cx.notify();
                        }))
                        .child(
                            svg()
                                .path(LucideIcons::MicVocal)
                                .size_5()
                                .text_color(rgb(ACCENT)),
                        ),
                )
                .child(
                    icon_button("queue", LucideIcons::ListMusic, true).on_click(cx.listener(
                        |app, _, _, cx| {
                            app.navigation.navigate(Page::Queue);
                            cx.notify();
                        },
                    )),
                )
                .child(
                    icon_button(
                        "volume",
                        if muted {
                            LucideIcons::VolumeX
                        } else {
                            LucideIcons::Volume2
                        },
                        true,
                    )
                    .on_click(cx.listener(|app, _, _, _| {
                        app.commands
                            .set_volume(if app.playback.volume <= f64::EPSILON {
                                1.0
                            } else {
                                0.0
                            });
                    })),
                ),
        )
}

pub fn render_now_playing(app: &mut HmpGpuiApp, cx: &mut gpui::Context<HmpGpuiApp>) -> AnyElement {
    let playing = is_playing(&app.playback);
    let progress = progress(&app.playback);
    let elapsed = elapsed_text(&app.playback);
    let remaining = app
        .playback
        .duration
        .map(|_| format!("−{}", remaining_text(&app.playback)))
        .unwrap_or_else(|| "−:--".into());
    let shuffle = app.playback.shuffle;
    let repeat = repeat_icon_state(app.playback.loop_mode);
    let progress_bounds = Rc::new(Cell::<Option<Bounds<Pixels>>>::new(None));
    let progress_bounds_for_paint = Rc::clone(&progress_bounds);
    let progress_bounds_for_click = Rc::clone(&progress_bounds);

    div()
        .w_full()
        .flex()
        .flex_col()
        .child(
            div()
                .id("now-playing-seek")
                .relative()
                .w_full()
                .h(px(16.))
                .when(app.playback.can_seek, |bar| bar.cursor_pointer())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |app, event: &MouseDownEvent, _, _| {
                        let (Some(duration), Some(bounds)) =
                            (app.playback.duration, progress_bounds_for_click.get())
                        else {
                            return;
                        };
                        if app.playback.can_seek {
                            app.commands.seek(seek_seconds_for_click(
                                duration,
                                event.position.x.as_f32(),
                                bounds.origin.x.as_f32(),
                                bounds.size.width.as_f32(),
                            ));
                        }
                    }),
                )
                .child(
                    canvas(
                        move |bounds, _, _| {
                            progress_bounds_for_paint.set(Some(bounds));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(7.))
                        .h(px(4.))
                        .rounded_full()
                        .bg(rgba(0xffffff4c)),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(7.))
                        .w(relative(progress))
                        .h(px(4.))
                        .rounded_full()
                        .bg(rgba(0xffffffe0)),
                ),
        )
        .child(
            div()
                .mt(px(-1.))
                .flex()
                .justify_between()
                .text_size(px(9.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgba(0xffffff72))
                .child(elapsed)
                .child(remaining),
        )
        .child(
            div()
                .mt(px(5.))
                .h(px(48.))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    now_playing_icon_button(
                        "now-playing-shuffle",
                        LucideIcons::Shuffle,
                        17.,
                        shuffle,
                    )
                    .on_click(cx.listener(|app, _, _, _| {
                        app.commands.set_shuffle(!app.playback.shuffle);
                    })),
                )
                .child(
                    now_playing_icon_button(
                        "now-playing-skip-back",
                        LucideIcons::SkipBack,
                        24.,
                        true,
                    )
                    .on_click(cx.listener(|app, _, _, _| app.commands.previous())),
                )
                .child(
                    div()
                        .id("now-playing-play-pause")
                        .size(px(46.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(0xffffff12)))
                        .on_click(cx.listener(|app, _, _, _| app.commands.toggle_play()))
                        .child(
                            svg()
                                .path(if playing {
                                    LucideIcons::Pause
                                } else {
                                    LucideIcons::Play
                                })
                                .size(px(31.))
                                .text_color(rgba(0xffffffff)),
                        ),
                )
                .child(
                    now_playing_icon_button(
                        "now-playing-skip-forward",
                        LucideIcons::SkipForward,
                        24.,
                        true,
                    )
                    .on_click(cx.listener(|app, _, _, _| app.commands.next())),
                )
                .child(
                    now_playing_icon_button(
                        "now-playing-repeat",
                        if repeat == RepeatIconState::One {
                            LucideIcons::Repeat1
                        } else {
                            LucideIcons::Repeat
                        },
                        17.,
                        repeat != RepeatIconState::Off,
                    )
                    .on_click(cx.listener(|app, _, _, _| {
                        app.commands
                            .set_loop_mode(next_loop_mode(app.playback.loop_mode));
                    })),
                ),
        )
        .into_any_element()
}

fn seek_seconds_for_click(duration: Duration, click_x: f32, left: f32, width: f32) -> f32 {
    if width <= f32::EPSILON {
        return 0.0;
    }
    duration.as_secs_f32() * ((click_x - left) / width).clamp(0.0, 1.0)
}

fn now_playing_icon_button(
    id: &'static str,
    icon: LucideIcons,
    icon_size: f32,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(42.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xffffff12)))
        .child(svg().path(icon).size(px(icon_size)).text_color(if enabled {
            rgba(0xffffffff)
        } else {
            rgba(0xffffffb0)
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_target_uses_clicked_progress_ratio_and_clamps_edges() {
        let duration = std::time::Duration::from_secs(200);
        assert_eq!(seek_seconds_for_click(duration, 125.0, 25.0, 200.0), 100.0);
        assert_eq!(seek_seconds_for_click(duration, -50.0, 25.0, 200.0), 0.0);
        assert_eq!(seek_seconds_for_click(duration, 500.0, 25.0, 200.0), 200.0);
        assert_eq!(seek_seconds_for_click(duration, 25.0, 25.0, 0.0), 0.0);
    }
}
