//! Top bar visuals adapted from `cradiy/gpui-apple-music-demo` (MIT).

use gpui::{WindowControlArea, div, prelude::*, px, rgba, svg};
use uic::{
    assets::LucideIcons,
    components::input::{Input, InputAppearance},
};

use crate::{app::HmpGpuiApp, theme::layout};

fn appearance() -> InputAppearance {
    InputAppearance {
        placeholder: rgba(0xe2e2e7c2).into(),
        focus_border: rgba(0xec4168d9).into(),
        caret: rgba(0xf14367cf).into(),
        selection: rgba(0xec41683d).into(),
        caret_width: px(1.),
        caret_height: px(17.),
    }
}

fn drag_region() -> impl IntoElement {
    div()
        .h_full()
        .flex_1()
        .window_control_area(WindowControlArea::Drag)
}

pub fn render(app: &mut HmpGpuiApp, compact: bool) -> impl IntoElement {
    div()
        .h(px(layout::TOP_BAR_HEIGHT))
        .px(px(18.))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgba(0xffffff08))
        .child(drag_region())
        .child(
            div()
                .w(if compact {
                    px(layout::TOP_BAR_SEARCH_WIDTH_COMPACT)
                } else {
                    px(layout::TOP_BAR_SEARCH_WIDTH)
                })
                .h(px(35.))
                .flex_none()
                .rounded(px(16.))
                .shadow(vec![
                    gpui::BoxShadow::new(px(0.), px(0.), rgba(0xe7325a42).into())
                        .blur_radius(px(8.)),
                ])
                .child(
                    Input::new(&app.search_input)
                        .appearance(appearance())
                        .h(px(35.))
                        .px(px(11.))
                        .gap(px(7.))
                        .text_size(px(10.5))
                        .text_color(rgba(0xe2e2e7e8))
                        .rounded(px(16.))
                        .border(px(2.))
                        .border_color(rgba(0xffffff16))
                        .bg(rgba(0x12141cbf))
                        .prefix(
                            svg()
                                .path(LucideIcons::Search)
                                .size(px(14.))
                                .text_color(rgba(0xd4d4dcc4)),
                        ),
                ),
        )
        .child(drag_region())
}
