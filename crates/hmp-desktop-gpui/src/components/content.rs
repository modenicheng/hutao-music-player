//! Empty page states adapted from `cradiy/gpui-apple-music-demo` (MIT).

use gpui::{AnyElement, FontWeight, div, prelude::*, px, rgba, svg};
use uic::assets::LucideIcons;

use crate::{app::HmpGpuiApp, state::Page};

pub fn render(app: &mut HmpGpuiApp, cx: &mut gpui::Context<HmpGpuiApp>) -> AnyElement {
    match app.navigation.page {
        Page::Search => render_search(app, cx),
        page => render_empty(page),
    }
}

fn render_empty(page: Page) -> AnyElement {
    let (title, subtitle, icon) = match page {
        Page::Search => (
            "搜索音乐",
            "输入歌曲、歌手或专辑名称，结果将由 HMP Core 提供。",
            LucideIcons::Search,
        ),
        Page::Recommend => (
            "推荐",
            "你的个性化推荐将在这里显示。",
            LucideIcons::Sparkles,
        ),
        Page::Library => (
            "媒体库",
            "本地音乐与收藏内容将在这里显示。",
            LucideIcons::Library,
        ),
        Page::Queue => ("播放队列", "当前队列将在这里显示。", LucideIcons::ListMusic),
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
            div()
                .id(("search-result", index))
                .h(px(58.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(12.))
                .rounded(px(10.))
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0xffffff0d)))
                .on_click(cx.listener(move |app, _, _, _| {
                    app.commands.play_search_result(index);
                }))
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
                                .path(LucideIcons::Music2)
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
                                .child(artist),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgba(0xb7b8c270))
                        .child(duration),
                )
        });

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
                .child("搜索"),
        )
        .child(
            div()
                .mt(px(5.))
                .mb(px(15.))
                .text_size(px(10.5))
                .text_color(rgba(0xb5b6c078))
                .child("按 Enter 将关键词提交给 HMP Core"),
        )
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
            content.child(
                div()
                    .p(px(12.))
                    .rounded(px(9.))
                    .bg(rgba(0x7f20305c))
                    .text_size(px(11.))
                    .text_color(rgba(0xffc6cddd))
                    .child(message),
            )
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
