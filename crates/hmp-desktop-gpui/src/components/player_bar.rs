//! Player bar visuals adapted from `cradiy/gpui-apple-music-demo` (MIT).
//! All playback actions are routed through HMP `AppCommand`.

use gpui::{Div, FontWeight, ObjectFit, Stateful, div, img, prelude::*, px, relative, rgba, svg};
use uic::assets::LucideIcons;

use crate::{
    app::HmpGpuiApp,
    bridge::{elapsed_text, is_playing, progress, remaining_text},
    theme::{ACCENT, TEXT_PRIMARY},
};

fn icon_button(id: &'static str, icon: LucideIcons, enabled: bool, size: f32) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0xffffff12)))
        })
        .child(svg().path(icon).size(px(size)).text_color(if enabled {
            rgba(0xf5f5fae8)
        } else {
            rgba(0xa8a9b34d)
        }))
}

pub fn render(
    app: &mut HmpGpuiApp,
    cx: &mut gpui::Context<HmpGpuiApp>,
    compact: bool,
) -> impl IntoElement {
    let playback = &app.playback;
    let playing = is_playing(playback);
    let has_track = playback.current.is_some();
    let has_queue = !app.events.queue.is_empty();
    let progress = progress(playback);
    let volume = playback.volume.clamp(0.0, 1.0) as f32;
    let elapsed = elapsed_text(playback);
    let remaining = remaining_text(playback);
    let title = playback
        .current
        .as_ref()
        .map(|track| track.title.clone())
        .unwrap_or_else(|| "尚未播放".into());
    let artist = playback
        .current
        .as_ref()
        .map(|track| track.artist_names())
        .filter(|artist| !artist.is_empty())
        .unwrap_or_else(|| "从搜索或媒体库选择歌曲".into());
    let cover_url = playback
        .current
        .as_ref()
        .and_then(|track| track.cover.as_ref())
        .map(|cover| cover.url.clone());
    let has_cover = cover_url.is_some();

    div()
        .relative()
        .w(px(if compact { 620. } else { 720. }))
        .h(px(76.))
        .px(px(14.))
        .flex()
        .items_center()
        .gap(px(14.))
        .rounded(px(25.))
        .bg(rgba(0x181a23e2))
        .backdrop_blur(px(22.))
        .border_1()
        .border_color(rgba(0xffffff12))
        .shadow(vec![
            gpui::BoxShadow::new(px(0.), px(9.), rgba(0x00000051).into())
                .blur_radius(px(22.))
                .spread_radius(px(-7.)),
        ])
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    icon_button("previous", LucideIcons::SkipBack, has_queue, 18.).on_click(
                        cx.listener(|app, _, _, _| {
                            app.commands.previous();
                        }),
                    ),
                )
                .child(
                    div()
                        .id("play-pause")
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(rgba(0xffffff13))
                        .hover(|style| style.bg(rgba(0xffffff22)))
                        .on_click(cx.listener(|app, _, _, _| {
                            app.commands.toggle_play();
                        }))
                        .child(
                            svg()
                                .path(if playing {
                                    LucideIcons::Pause
                                } else {
                                    LucideIcons::Play
                                })
                                .size(px(22.))
                                .text_color(if has_track {
                                    rgba(0xffffffff)
                                } else {
                                    rgba(0xb9bac36d)
                                }),
                        ),
                )
                .child(
                    icon_button("next", LucideIcons::SkipForward, has_queue, 18.).on_click(
                        cx.listener(|app, _, _, _| {
                            app.commands.next();
                        }),
                    ),
                ),
        )
        .child(
            div()
                .id("open-now-playing")
                .relative()
                .size(px(46.))
                .flex_none()
                .overflow_hidden()
                .rounded(px(10.))
                .cursor_pointer()
                .border_1()
                .border_color(rgba(0xffffff18))
                .bg(rgba(0x333542ff))
                .when_some(cover_url, |cover, url| {
                    cover.child(
                        img(url)
                            .size_full()
                            .rounded(px(10.))
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .when(!has_cover, |cover| {
                    cover.child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                svg()
                                    .path(LucideIcons::Music2)
                                    .size(px(22.))
                                    .text_color(rgba(0xffffff54)),
                            ),
                    )
                })
                .on_click(cx.listener(|app, _, _, cx| {
                    if app.playback.current.is_some() {
                        app.now_playing = true;
                        cx.notify();
                    }
                })),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba((TEXT_PRIMARY << 8) | 0xe8))
                                .child(title),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(9.5))
                                .text_color(rgba(0xb7b8c288))
                                .child(artist),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(px(9.))
                        .text_color(rgba(0xffffff72))
                        .child(elapsed)
                        .child(
                            div()
                                .id("seek-forward")
                                .relative()
                                .h(px(12.))
                                .flex_1()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .on_click(cx.listener(|app, _, _, _| {
                                    if !app.playback.can_seek {
                                        return;
                                    }
                                    let target = app
                                        .playback
                                        .duration
                                        .map(|duration| {
                                            app.playback
                                                .position
                                                .saturating_add(std::time::Duration::from_secs(10))
                                                .min(duration)
                                        })
                                        .unwrap_or(app.playback.position);
                                    app.commands.seek(target.as_secs_f32());
                                }))
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .h(px(3.))
                                        .rounded_full()
                                        .bg(rgba(0xffffff2c)),
                                )
                                .child(
                                    div()
                                        .h(px(3.))
                                        .w(relative(progress))
                                        .rounded_full()
                                        .bg(rgba((ACCENT << 8) | 0xff)),
                                ),
                        )
                        .child(remaining),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(
                    icon_button("volume-down", LucideIcons::Volume1, true, 15.).on_click(
                        cx.listener(|app, _, _, _| {
                            app.commands
                                .set_volume((app.playback.volume as f32 - 0.1).clamp(0.0, 1.0));
                        }),
                    ),
                )
                .child(
                    div()
                        .w(px(44.))
                        .h(px(3.))
                        .rounded_full()
                        .bg(rgba(0xffffff2c))
                        .child(
                            div()
                                .h_full()
                                .w(relative(volume))
                                .rounded_full()
                                .bg(rgba(0xffffffc8)),
                        ),
                )
                .child(
                    icon_button("volume-up", LucideIcons::Volume2, true, 15.).on_click(
                        cx.listener(|app, _, _, _| {
                            app.commands
                                .set_volume((app.playback.volume as f32 + 0.1).clamp(0.0, 1.0));
                        }),
                    ),
                ),
        )
}
