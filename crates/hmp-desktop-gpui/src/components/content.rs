//! HMP page bodies rendered inside the reference demo's single content slot.

use gpui::{AnyElement, FontWeight, div, prelude::*, px, rgba, svg};
use uic::assets::LucideIcons;

use crate::{app::HmpGpuiApp, state::Page};

pub const CONTENT_ROW_HEIGHT: f32 = 58.0;

pub fn render(app: &mut HmpGpuiApp, cx: &mut gpui::Context<HmpGpuiApp>) -> AnyElement {
    match app.navigation.page {
        Page::Search => render_search(app, cx),
        Page::Queue => render_queue(app, cx),
        Page::Library => match app.navigation.selected_playlist_id {
            Some(playlist_id) => render_playlist(app, playlist_id, cx),
            None => render_empty(Page::Library),
        },
        page => render_empty(page),
    }
}

fn render_empty(page: Page) -> AnyElement {
    let (title, subtitle, icon) = match page {
        Page::Search => (
            "搜索音乐",
            "输入歌曲、歌手或专辑名称。",
            LucideIcons::Search,
        ),
        Page::Recommend => (
            "推荐",
            "你的个性化推荐将在这里显示。",
            LucideIcons::Sparkles,
        ),
        Page::Library => ("媒体库", "从侧栏选择一个 HMP 歌单。", LucideIcons::Library),
        Page::Queue => ("播放队列", "当前队列为空。", LucideIcons::ListMusic),
        Page::Lyrics => (
            "歌词",
            "播放歌曲后，这里会跟随 HMP 播放状态同步歌词。",
            LucideIcons::Captions,
        ),
        Page::Settings => (
            "设置",
            "桌面外观和播放偏好将在这里配置。",
            LucideIcons::Settings,
        ),
    };

    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .mt(px(-28.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(svg().path(icon).size(px(48.)).text_color(rgba(0xd6d6dc72)))
                .child(
                    div()
                        .text_size(px(21.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(0xd8d8deaa))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgba(0xb5b6c071))
                        .child(subtitle),
                ),
        )
        .into_any_element()
}

fn list_page(title: impl IntoElement, subtitle: impl IntoElement) -> gpui::Div {
    div()
        .flex_1()
        .min_h_0()
        .px(px(28.))
        .pt(px(24.))
        .pb(px(112.))
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(24.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(0xf3f3f6eb))
                .child(title),
        )
        .child(
            div()
                .mt(px(5.))
                .mb(px(15.))
                .text_size(px(10.5))
                .text_color(rgba(0xb5b6c078))
                .child(subtitle),
        )
}

fn error_banner(message: String) -> AnyElement {
    div()
        .mb(px(10.))
        .p(px(12.))
        .rounded(px(9.))
        .bg(rgba(0x7f20305c))
        .text_size(px(11.))
        .text_color(rgba(0xffc6cddd))
        .child(message)
        .into_any_element()
}

fn render_search(app: &mut HmpGpuiApp, cx: &mut gpui::Context<HmpGpuiApp>) -> AnyElement {
    let loading = app.events.search_loading;
    let error = app.events.search_error.clone();
    let rows = app
        .events
        .search_results
        .iter()
        .enumerate()
        .map(|(index, song)| {
            let title = song.title.clone();
            let artist = song.artist.clone();
            let duration = song.duration.clone();
            music_row(
                ("search-result", index),
                title,
                artist,
                String::new(),
                duration,
                false,
            )
            .on_click(cx.listener(move |app, _, _, _| {
                app.commands.play_search_result(index);
            }))
        });

    list_page("搜索", "按 Enter 将关键词提交给 HMP Core")
        .when(loading, |content| {
            content.child(
                div()
                    .py(px(18.))
                    .text_size(px(11.))
                    .text_color(rgba(0xd8d8de90))
                    .child("正在搜索…"),
            )
        })
        .when_some(error, |content, message| {
            content.child(error_banner(message))
        })
        .when(
            !loading && app.events.search_results.is_empty(),
            |content| {
                content.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.))
                        .text_color(rgba(0xb5b6c071))
                        .child("输入歌曲、歌手或专辑名称开始搜索"),
                )
            },
        )
        .child(
            div()
                .id("search-results")
                .min_h_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .overflow_y_scroll()
                .children(rows),
        )
        .into_any_element()
}

fn render_queue(app: &mut HmpGpuiApp, cx: &mut gpui::Context<HmpGpuiApp>) -> AnyElement {
    if app.events.queue.is_empty() {
        return render_empty(Page::Queue);
    }
    let rows = app.events.queue.iter().enumerate().map(|(index, item)| {
        music_row(
            ("queue-row", index),
            item.title.clone(),
            item.artist.clone(),
            String::new(),
            item.duration.clone(),
            item.is_current,
        )
        .on_click(cx.listener(move |app, _, _, _| {
            app.commands.play_queue_item(index);
        }))
    });

    list_page("播放队列", format!("{} 首曲目", app.events.queue.len()))
        .child(
            div()
                .id("queue-rows")
                .min_h_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .overflow_y_scroll()
                .children(rows),
        )
        .into_any_element()
}

fn render_playlist(
    app: &mut HmpGpuiApp,
    playlist_id: i64,
    cx: &mut gpui::Context<HmpGpuiApp>,
) -> AnyElement {
    let playlist = app
        .events
        .playlists
        .iter()
        .find(|playlist| playlist.id == playlist_id);
    let title = playlist
        .map(|playlist| playlist.name.clone())
        .unwrap_or_else(|| "歌单".into());
    let subtitle = playlist
        .map(|playlist| {
            format!(
                "{} 首曲目 · {} · {}",
                playlist.track_count, playlist.relation, playlist.sync_state
            )
        })
        .unwrap_or_else(|| "正在读取 HMP 媒体库".into());
    let error = app.events.playlist_error.clone();
    let loaded = app.events.selected_playlist == Some(playlist_id);
    let rows = loaded
        .then(|| {
            app.events
                .playlist_tracks
                .iter()
                .enumerate()
                .map(|(index, track)| {
                    music_row(
                        ("playlist-track", index),
                        track.title.clone(),
                        track.artist.clone(),
                        track.album.clone(),
                        track.duration.clone(),
                        false,
                    )
                    .on_click(cx.listener(move |app, _, _, _| {
                        app.commands.play_playlist_track(playlist_id, index);
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    list_page(title, subtitle)
        .when_some(error, |content, message| {
            content.child(error_banner(message))
        })
        .when(!loaded, |content| {
            content.child(
                div()
                    .py(px(18.))
                    .text_size(px(11.))
                    .text_color(rgba(0xd8d8de90))
                    .child("正在载入歌单…"),
            )
        })
        .when(loaded && rows.is_empty(), |content| {
            content.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .text_color(rgba(0xb5b6c071))
                    .child("这个歌单还没有曲目"),
            )
        })
        .child(
            div()
                .id(("playlist-tracks", playlist_id as usize))
                .min_h_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .overflow_y_scroll()
                .children(rows),
        )
        .into_any_element()
}

fn music_row(
    id: impl Into<gpui::ElementId>,
    title: String,
    artist: String,
    album: String,
    duration: String,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let detail = match (artist.is_empty(), album.is_empty()) {
        (false, false) => format!("{artist} · {album}"),
        (false, true) => artist,
        (true, false) => album,
        (true, true) => "未知艺术家".into(),
    };
    div()
        .id(id)
        .h(px(CONTENT_ROW_HEIGHT))
        .px(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(12.))
        .rounded(px(10.))
        .cursor_pointer()
        .when(selected, |row| row.bg(rgba(0xffffff10)))
        .hover(|style| style.bg(rgba(0xffffff0d)))
        .child(
            div()
                .size(px(38.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(9.))
                .bg(rgba(0xffffff0b))
                .child(
                    svg()
                        .path(if selected {
                            LucideIcons::AudioLines
                        } else {
                            LucideIcons::Music2
                        })
                        .size(px(18.))
                        .text_color(rgba(0xf14367c8)),
                ),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgba(0xf1f1f4e8))
                        .child(title),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(10.))
                        .text_color(rgba(0xb7b8c288))
                        .child(detail),
                ),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgba(0xb7b8c270))
                .child(duration),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_search_and_playlist_rows_keep_reference_rhythm() {
        assert_eq!(CONTENT_ROW_HEIGHT, 58.0);
    }
}
