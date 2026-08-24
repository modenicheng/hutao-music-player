//! Sidebar visuals adapted from `cradiy/gpui-apple-music-demo` (MIT).

use gpui::{FontWeight, WindowControlArea, div, prelude::*, px, rgb, rgba, svg};
use gpui_effects::{FrostedGlass, FrostedGlassAppearance};
use uic::assets::LucideIcons;

use crate::{app::HmpGpuiApp, state::Page, theme::layout};

fn traffic_light(color: u32) -> impl IntoElement {
    div()
        .size(px(12.))
        .rounded_full()
        .bg(rgb(color))
        .border_1()
        .border_color(rgba(0x0000002b))
}

fn nav_section(label: &'static str) -> impl IntoElement {
    div()
        .h(px(28.))
        .px(px(19.))
        .pt(px(10.))
        .flex()
        .items_center()
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgba(0xa5a7b5a8))
        .child(label)
}

fn nav_item(
    id: &'static str,
    icon: LucideIcons,
    page: Page,
    selected: bool,
    cx: &mut gpui::Context<HmpGpuiApp>,
) -> impl IntoElement {
    div()
        .id(id)
        .mx(px(9.))
        .h(px(35.))
        .px(px(9.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(15.))
        .font_weight(if selected {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .text_color(if selected {
            rgba(0xfff8f9f2)
        } else {
            rgba(0xe8e8eee0)
        })
        .when(selected, |row| row.bg(rgba(0x3a3b4a8c)))
        .when(!selected, |row| {
            row.hover(|style| style.bg(rgba(0xffffff10)))
        })
        .on_click(cx.listener(move |app, _, _, cx| {
            app.navigation.navigate(page);
            cx.notify();
        }))
        .child(svg().path(icon).size(px(16.)).text_color(if selected {
            rgba(0xff3860ff)
        } else {
            rgba(0xf0f0f5dc)
        }))
        .child(page.label())
}

pub fn render(app: &mut HmpGpuiApp, cx: &mut gpui::Context<HmpGpuiApp>) -> impl IntoElement {
    let glass = FrostedGlassAppearance::dark()
        .blur_radius(px(15.))
        .saturation(1.8)
        .brightness(1.12)
        .tint(rgba(0x17102f78).into())
        .edge(rgba(0x7656d48f).into())
        .edge_width(px(1.))
        .sheen(0.2);

    FrostedGlass::with_appearance(glass)
        .rounded(px(25.))
        .w(px(layout::SIDEBAR_WIDTH + layout::SIDEBAR_GUTTER))
        .flex_1()
        .border_1()
        .border_color(rgba(0x7656d48f))
        .child(
            div()
                .h(px(41.))
                .px(px(15.))
                .flex()
                .items_center()
                .gap(px(8.))
                .window_control_area(WindowControlArea::Drag)
                .when(cfg!(not(target_os = "macos")), |header| {
                    header
                        .child(traffic_light(0xff5f57))
                        .child(traffic_light(0xffbd2e))
                        .child(traffic_light(0x28c840))
                }),
        )
        .child(nav_item(
            "nav-search",
            LucideIcons::Search,
            Page::Search,
            app.navigation.page == Page::Search,
            cx,
        ))
        .child(nav_item(
            "nav-recommend",
            LucideIcons::Sparkles,
            Page::Recommend,
            app.navigation.page == Page::Recommend,
            cx,
        ))
        .child(nav_item(
            "nav-library",
            LucideIcons::Library,
            Page::Library,
            app.navigation.page == Page::Library,
            cx,
        ))
        .child(nav_section("正在播放"))
        .child(nav_item(
            "nav-queue",
            LucideIcons::ListMusic,
            Page::Queue,
            app.navigation.page == Page::Queue,
            cx,
        ))
        .child(nav_item(
            "nav-lyrics",
            LucideIcons::Captions,
            Page::Lyrics,
            app.navigation.page == Page::Lyrics,
            cx,
        ))
        .child(nav_section("HMP"))
        .child(nav_item(
            "nav-settings",
            LucideIcons::Settings,
            Page::Settings,
            app.navigation.page == Page::Settings,
            cx,
        ))
}
