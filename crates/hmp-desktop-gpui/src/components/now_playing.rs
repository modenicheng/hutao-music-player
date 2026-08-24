//! Immersive Now Playing surface adapted from `cradiy/gpui-apple-music-demo` (MIT).
//! HMP metadata, queue actions, lyrics, and playback commands remain authoritative.

use std::{sync::Arc, time::Instant};

use gpui::{
    AnyElement, Div, FontWeight, Image, ImageFormat, ImageSource, ObjectFit, Stateful, div, img,
    prelude::*, px, rgba, svg,
};
use gpui_effects::album_glow;
use uic::assets::LucideIcons;

use crate::{
    app::{EffectsViewer, HmpGpuiApp},
    components::{lyrics_panel, player_bar},
    theme::layout,
};

const FALLBACK_COVER: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="800" viewBox="0 0 800 800">
<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop stop-color="#ff375f"/><stop offset=".5" stop-color="#6e5bff"/><stop offset="1" stop-color="#10131d"/></linearGradient></defs>
<rect width="800" height="800" rx="120" fill="url(#g)"/><path d="M510 175v360c0 74-60 134-134 134s-134-60-134-134 60-134 134-134c28 0 54 9 75 23V232l59-57z" fill="white" fill-opacity=".9"/>
</svg>"##;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NowPlayingStage {
    Lyrics,
    Queue,
}

pub const fn now_playing_stage(show_lyrics: bool) -> NowPlayingStage {
    if show_lyrics {
        NowPlayingStage::Lyrics
    } else {
        NowPlayingStage::Queue
    }
}

pub fn render(
    app: &mut HmpGpuiApp,
    window_width: f32,
    window_height: f32,
    cx: &mut gpui::Context<HmpGpuiApp>,
) -> AnyElement {
    let track = app.playback.current.as_ref();
    let cover_key = track
        .map(|track| track.id.to_string())
        .unwrap_or_else(|| "hmp-fallback-cover".to_owned());
    let artwork: ImageSource = track
        .and_then(|track| track.cover.as_ref())
        .map(|cover| cover.url.clone().into())
        .unwrap_or_else(|| fallback_cover().into());

    let reset_effect = app
        .effects_viewer
        .as_ref()
        .is_none_or(|effect| effect.cover_key != cover_key);
    if reset_effect {
        app.effects_viewer = Some(EffectsViewer {
            cover_key,
            started_at: Instant::now(),
        });
    }
    let background_time = app
        .effects_viewer
        .as_ref()
        .map(|effect| effect.started_at.elapsed().as_secs_f32())
        .unwrap_or_default();

    let title = track
        .map(|track| track.title.clone())
        .unwrap_or_else(|| "尚未播放".to_owned());
    let artist = track
        .map(|track| track.artist_names())
        .filter(|artist| !artist.is_empty())
        .unwrap_or_else(|| "HMP".to_owned());
    let album = track
        .and_then(|track| track.album.as_ref())
        .map(|album| album.name.clone());
    let subtitle = album
        .map(|album| format!("{artist} — {album}"))
        .unwrap_or(artist);
    let stage = now_playing_stage(app.now_playing_lyrics);
    let cover_size = layout::now_playing_cover_size(window_width, window_height);
    let side_panel = match stage {
        NowPlayingStage::Lyrics => {
            lyrics_panel::render_fullscreen(&app.playback, &app.events, &mut app.lyrics_panel)
        }
        NowPlayingStage::Queue => render_queue(app, cx),
    };

    div()
        .relative()
        .size_full()
        .when(cfg!(target_os = "linux"), |root| {
            root.font_family(crate::theme::FONT_FAMILY)
        })
        .overflow_hidden()
        .rounded(px(22.))
        .text_color(rgba(0xffffffff))
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .left_0()
                .overflow_hidden()
                .child(
                    album_glow(artwork.clone())
                        .time(background_time)
                        .size_full(),
                ),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .left_0()
                .bg(rgba(0x080a102b)),
        )
        .child(
            div()
                .relative()
                .size_full()
                .px(px(72.))
                .pt(px(82.))
                .pb(px(62.))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(92.))
                .child(
                    div()
                        .w(px(cover_size))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .size(px(cover_size))
                                .flex_none()
                                .overflow_hidden()
                                .rounded_3xl()
                                .shadow(vec![
                                    gpui::BoxShadow::new(px(0.), px(24.), rgba(0x0000005e).into())
                                        .blur_radius(px(52.))
                                        .spread_radius(px(-15.)),
                                ])
                                .child(
                                    img(artwork)
                                        .size_full()
                                        .rounded_3xl()
                                        .object_fit(ObjectFit::Cover),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .mt(px(18.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(14.))
                                                .font_weight(FontWeight::BOLD)
                                                .child(title),
                                        )
                                        .child(
                                            div()
                                                .mt(px(3.))
                                                .truncate()
                                                .text_size(px(11.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgba(0xffffffad))
                                                .child(subtitle),
                                        ),
                                )
                                .child(
                                    div()
                                        .ml(px(16.))
                                        .flex()
                                        .items_center()
                                        .gap(px(7.))
                                        .child(round_icon_button(
                                            "favorite-track",
                                            LucideIcons::Star,
                                            15.,
                                        ))
                                        .child(round_icon_button(
                                            "track-actions",
                                            LucideIcons::Ellipsis,
                                            17.,
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .mt(px(18.))
                                .child(player_bar::render_now_playing(app, cx)),
                        ),
                )
                .child(
                    div()
                        .h_full()
                        .flex_1()
                        .min_w(px(390.))
                        .max_w(px(650.))
                        .child(side_panel),
                ),
        )
        .child(render_window_controls(cx))
        .child(render_view_switcher(app, cx))
        .into_any_element()
}

fn render_queue(app: &mut HmpGpuiApp, cx: &mut gpui::Context<HmpGpuiApp>) -> AnyElement {
    let rows = app
        .events
        .queue
        .clone()
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            div()
                .id(("now-playing-queue-row", index))
                .w_full()
                .px(px(16.))
                .py(px(11.))
                .flex()
                .items_center()
                .gap(px(12.))
                .rounded(px(12.))
                .cursor_pointer()
                .when(item.is_current, |row| row.bg(rgba(0xffffff18)))
                .hover(|style| style.bg(rgba(0xffffff16)))
                .on_click(cx.listener(move |app, _, _, _| {
                    app.commands.play_queue_item(index);
                }))
                .child(
                    div()
                        .w(px(22.))
                        .flex_none()
                        .text_center()
                        .text_size(px(10.))
                        .text_color(if item.is_current {
                            rgba(0xffffffff)
                        } else {
                            rgba(0xffffff70)
                        })
                        .child(if item.is_playing {
                            "▶".to_owned()
                        } else {
                            (index + 1).to_string()
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .font_weight(if item.is_current {
                                    FontWeight::BOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .child(item.title),
                        )
                        .child(
                            div()
                                .mt(px(3.))
                                .truncate()
                                .text_size(px(10.))
                                .text_color(rgba(0xffffff78))
                                .child(item.artist),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(9.))
                        .text_color(rgba(0xffffff70))
                        .child(item.duration),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();

    div()
        .h_full()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(70.))
                .px(px(18.))
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .child("接下来播放")
                .child(
                    div()
                        .text_size(px(9.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgba(0xffffff72))
                        .child(format!("{} 首", rows.len())),
                ),
        )
        .child(
            div()
                .id("now-playing-queue-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(4.))
                .when(rows.is_empty(), |list| {
                    list.items_center().justify_center().child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgba(0xffffff78))
                            .child("播放队列为空"),
                    )
                })
                .children(rows),
        )
        .into_any_element()
}

fn render_window_controls(cx: &mut gpui::Context<HmpGpuiApp>) -> AnyElement {
    div()
        .absolute()
        .top(px(18.))
        .left(px(18.))
        .right(px(18.))
        .h(px(34.))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(
                    div()
                        .w(px(52.))
                        .h(px(12.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .when(cfg!(not(target_os = "macos")), |lights| {
                            lights
                                .child(traffic_light(0xff5f57))
                                .child(traffic_light(0xfebc2e))
                                .child(traffic_light(0x28c840))
                        }),
                )
                .child(
                    div()
                        .h(px(32.))
                        .px(px(7.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded_full()
                        .bg(rgba(0xffffff2a))
                        .border_1()
                        .border_color(rgba(0xffffff1a))
                        .child(
                            top_icon_button("close-now-playing", LucideIcons::X).on_click(
                                cx.listener(|app, _, _, cx| {
                                    app.now_playing = false;
                                    app.effects_viewer = None;
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(top_icon_button(
                            "picture-in-picture",
                            LucideIcons::PictureInPicture2,
                        )),
                ),
        )
        .into_any_element()
}

fn render_view_switcher(app: &mut HmpGpuiApp, cx: &mut gpui::Context<HmpGpuiApp>) -> AnyElement {
    div()
        .absolute()
        .right(px(18.))
        .bottom(px(16.))
        .h(px(36.))
        .px(px(5.))
        .flex()
        .items_center()
        .gap(px(12.))
        .rounded_full()
        .bg(rgba(0xffffff2a))
        .border_1()
        .border_color(rgba(0xffffff1a))
        .child(
            view_switch_button(
                "toggle-now-playing-lyrics",
                LucideIcons::MicVocal,
                app.now_playing_lyrics,
            )
            .on_click(cx.listener(|app, _, _, cx| {
                app.now_playing_lyrics = true;
                cx.notify();
            })),
        )
        .child(
            view_switch_button(
                "toggle-now-playing-queue",
                LucideIcons::ListMusic,
                !app.now_playing_lyrics,
            )
            .on_click(cx.listener(|app, _, _, cx| {
                app.now_playing_lyrics = false;
                cx.notify();
            })),
        )
        .into_any_element()
}

fn view_switch_button(id: &'static str, icon: LucideIcons, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(27.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .when(active, |button| button.bg(rgba(0xffffff24)))
        .hover(|style| style.bg(rgba(0xffffff20)))
        .child(svg().path(icon).size(px(20.)).text_color(rgba(0xffffffff)))
}

fn round_icon_button(id: &'static str, icon: LucideIcons, icon_size: f32) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .bg(rgba(0xffffff13))
        .hover(|style| style.bg(rgba(0xffffff22)))
        .child(
            svg()
                .path(icon)
                .size(px(icon_size))
                .text_color(rgba(0xffffffe5)),
        )
}

fn top_icon_button(id: &'static str, icon: LucideIcons) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(25.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xffffff16)))
        .child(svg().path(icon).size(px(20.)).text_color(rgba(0xffffffff)))
}

fn traffic_light(color: u32) -> Div {
    div()
        .size(px(12.))
        .rounded_full()
        .bg(rgba((color << 8) | 0xff))
}

fn fallback_cover() -> Arc<Image> {
    Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        FALLBACK_COVER.as_bytes().to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_playing_switch_keeps_lyrics_and_queue_in_one_stage() {
        assert_eq!(now_playing_stage(true), NowPlayingStage::Lyrics);
        assert_eq!(now_playing_stage(false), NowPlayingStage::Queue);
    }
}
