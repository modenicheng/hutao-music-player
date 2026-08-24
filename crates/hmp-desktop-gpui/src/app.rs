//! GPUI application view. Business state remains outside this module.

use gpui::{AppContext, Render, Subscription, Window, div, prelude::*, px, rgb, rgba};
use uic::components::input::{InputEvent, TextInput};

use crate::{
    components::{content, sidebar, top_bar},
    state::{NavigationState, Page},
    theme::{BACKGROUND, layout},
};

pub struct HmpGpuiApp {
    pub navigation: NavigationState,
    pub search_input: gpui::Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

impl HmpGpuiApp {
    pub fn new(cx: &mut gpui::Context<Self>, window: &mut Window) -> Self {
        let search_input = cx.new(|cx| TextInput::new(cx).placeholder("搜索音乐"));
        let search_subscription = cx.subscribe(&search_input, |app, _, _event: &InputEvent, cx| {
            app.navigation.navigate(Page::Search);
            cx.notify();
        });
        cx.focus_view(&search_input, window);

        Self {
            navigation: NavigationState::default(),
            search_input,
            _subscriptions: vec![search_subscription],
        }
    }
}

impl Render for HmpGpuiApp {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        window.set_traffic_light_position(gpui::point(px(34.), px(34.)));

        let compact = window.bounds().size.width < px(layout::COMPACT_TOP_BAR_BELOW);

        div()
            .size_full()
            .when(cfg!(target_os = "linux"), |root| {
                root.font_family(crate::theme::FONT_FAMILY)
            })
            .flex()
            .overflow_hidden()
            .when(cfg!(not(target_os = "windows")), |root| {
                root.rounded(px(22.))
            })
            .bg(rgb(BACKGROUND))
            .border_1()
            .border_color(rgba(0x00000045))
            .shadow(vec![
                gpui::BoxShadow::new(px(0.), px(11.), rgba(0x00000042).into())
                    .blur_radius(px(28.))
                    .spread_radius(px(-7.)),
            ])
            .child(
                div()
                    .m_5()
                    .flex()
                    .flex_col()
                    .child(sidebar::render(self, cx)),
            )
            .child(
                div().flex_1().min_w_0().h_full().flex().child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .bg(rgb(0x20222d))
                        .child(top_bar::render(self, compact))
                        .child(content::render(self.navigation.page)),
                ),
            )
    }
}
