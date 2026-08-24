//! Empty page states adapted from `cradiy/gpui-apple-music-demo` (MIT).

use gpui::{FontWeight, div, prelude::*, px, rgba, svg};
use uic::assets::LucideIcons;

use crate::state::Page;

pub fn render(page: Page) -> impl IntoElement {
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
}
