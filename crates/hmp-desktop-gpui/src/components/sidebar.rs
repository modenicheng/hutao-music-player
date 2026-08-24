//! Reference-faithful sidebar adapted from `cradiy/gpui-apple-music-demo` (MIT).

use gpui::{
    AnyElement, FontWeight, WindowControlArea, div, linear_color_stop, linear_gradient, prelude::*,
    px, rgb, rgba, svg,
};
use gpui_effects::{FrostedGlass, FrostedGlassAppearance};
use hmp_desktop_common::{UiAuthData, UiLoginPhase, UiPlaylistData};
use uic::assets::LucideIcons;

use crate::{app::HmpGpuiApp, state::Page, theme::layout};

const PLAYLIST_COLORS: [(u32, u32); 6] = [
    (0xf6f7fb, 0xadafba),
    (0x403a73, 0xd8a8c0),
    (0x363fc5, 0xd66b91),
    (0x37b9d6, 0x234c91),
    (0xd7c3b0, 0x5b315c),
    (0xeb817b, 0x26233f),
];

#[cfg(test)]
pub const fn sidebar_pages() -> [Page; 6] {
    [
        Page::Search,
        Page::Recommend,
        Page::Library,
        Page::Queue,
        Page::Lyrics,
        Page::Settings,
    ]
}

pub fn account_label(auth: &UiAuthData) -> &str {
    if auth.phase == UiLoginPhase::LoggedIn && !auth.display_name.is_empty() {
        &auth.display_name
    } else {
        "登录 QQ 音乐"
    }
}

pub const fn playlist_viewport_slots(playlist_count: usize) -> usize {
    if playlist_count == 0 {
        1
    } else if playlist_count > 6 {
        6
    } else {
        playlist_count
    }
}

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

fn playlist_art(colors: (u32, u32)) -> impl IntoElement {
    div()
        .relative()
        .size(px(16.))
        .flex_none()
        .overflow_hidden()
        .rounded(px(3.))
        .bg(linear_gradient(
            135.,
            linear_color_stop(rgb(colors.0), 0.),
            linear_color_stop(rgb(colors.1), 1.),
        ))
}

fn playlist_item(
    playlist: UiPlaylistData,
    index: usize,
    cx: &mut gpui::Context<HmpGpuiApp>,
) -> AnyElement {
    let playlist_id = playlist.id;
    div()
        .id(("sidebar-playlist", playlist_id as usize))
        .mx(px(9.))
        .h(px(35.))
        .px(px(9.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(15.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgba(0xe9e9efdc))
        .hover(|style| style.bg(rgba(0xffffff10)))
        .on_click(cx.listener(move |app, _, _, cx| {
            app.navigation.navigate(Page::Library);
            app.commands.open_playlist(playlist_id);
            cx.notify();
        }))
        .child(playlist_art(PLAYLIST_COLORS[index % PLAYLIST_COLORS.len()]))
        .child(div().min_w_0().truncate().child(playlist.name))
        .into_any_element()
}

fn empty_playlist_item() -> AnyElement {
    div()
        .mx(px(9.))
        .h(px(35.))
        .px(px(9.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(6.))
        .text_size(px(15.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgba(0xb4b5bf72))
        .child(playlist_art(PLAYLIST_COLORS[0]))
        .child("暂无歌单")
        .into_any_element()
}

fn account_row(app: &mut HmpGpuiApp, cx: &mut gpui::Context<HmpGpuiApp>) -> AnyElement {
    let label = account_label(&app.events.auth).to_owned();
    let phase = app.events.auth.phase;
    div()
        .id("sidebar-account")
        .mx(px(9.))
        .mb(px(9.))
        .h(px(35.))
        .px(px(9.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(15.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgba(0xe9e9efdc))
        .hover(|style| style.bg(rgba(0xffffff10)))
        .on_click(cx.listener(move |app, _, _, cx| {
            app.events.open_login_modal();
            if phase != UiLoginPhase::LoggedIn {
                app.commands.start_login();
            }
            cx.notify();
        }))
        .child(
            svg()
                .path(LucideIcons::CircleUser)
                .size(px(16.))
                .text_color(rgba(0xf0f0f5dc)),
        )
        .child(div().min_w_0().truncate().child(label))
        .into_any_element()
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
    let playlist_rows = if app.events.playlists.is_empty() {
        vec![empty_playlist_item()]
    } else {
        app.events
            .playlists
            .clone()
            .into_iter()
            .enumerate()
            .map(|(index, playlist)| playlist_item(playlist, index, cx))
            .collect::<Vec<_>>()
    };

    FrostedGlass::with_appearance(glass)
        .rounded(px(25.))
        .w(px(layout::SIDEBAR_WIDTH + layout::SIDEBAR_GUTTER))
        .flex_1()
        .flex()
        .flex_col()
        .border_1()
        .border_color(rgba(0x7656d48f))
        .child(
            div()
                .h(px(41.))
                .px(px(15.))
                .flex_none()
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
        .child(nav_section("资料库"))
        .child(nav_item(
            "nav-library",
            LucideIcons::Library,
            Page::Library,
            app.navigation.page == Page::Library,
            cx,
        ))
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
        .child(nav_item(
            "nav-settings",
            LucideIcons::Settings,
            Page::Settings,
            app.navigation.page == Page::Settings,
            cx,
        ))
        .child(nav_section("歌单"))
        .child(
            div()
                .id("sidebar-playlists")
                .h(px(playlist_viewport_slots(6) as f32 * 35.0))
                .flex_none()
                .overflow_y_scroll()
                .children(playlist_rows),
        )
        .child(div().flex_1())
        .child(account_row(app, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_demo_navigation_slots_map_to_hmp_pages() {
        assert_eq!(
            sidebar_pages(),
            [
                Page::Search,
                Page::Recommend,
                Page::Library,
                Page::Queue,
                Page::Lyrics,
                Page::Settings,
            ]
        );
    }

    #[test]
    fn sidebar_account_copy_tracks_auth_phase() {
        assert_eq!(account_label(&UiAuthData::logged_out()), "登录 QQ 音乐");
        assert_eq!(account_label(&UiAuthData::logged_in("10001")), "10001");
    }

    #[test]
    fn sidebar_playlist_viewport_keeps_six_reference_slots() {
        assert_eq!(playlist_viewport_slots(0), 1);
        assert_eq!(playlist_viewport_slots(6), 6);
        assert_eq!(playlist_viewport_slots(12), 6);
    }
}
