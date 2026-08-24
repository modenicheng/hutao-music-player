//! HMP GPUI desktop prototype bootstrap.
//!
//! The window setup is adapted from `cradiy/gpui-apple-music-demo` (MIT),
//! while playback remains owned by HMP Core.

use std::process::ExitCode;

use gpui::{
    App, AppContext, Bounds, Render, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowOptions, div, point, prelude::*, px, rgb, size,
};
use gpui_platform::application;
use uic::assets::LucideAssets;

mod theme;
mod window;

struct HmpGpuiApp;

impl Render for HmpGpuiApp {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(theme::BACKGROUND))
            .text_color(rgb(theme::TEXT_PRIMARY))
            .child("Hutao Music Player")
    }
}

fn titlebar() -> Option<TitlebarOptions> {
    #[cfg(target_os = "macos")]
    {
        Some(TitlebarOptions {
            appears_transparent: true,
            traffic_light_position: Some(point(px(34.), px(34.))),
            ..Default::default()
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = point(px(0.), px(0.));
        None
    }
}

fn main() -> ExitCode {
    application()
        .with_assets(LucideAssets::new())
        .run(|cx: &mut App| {
            uic::components::input::init(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: titlebar(),
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |window, cx| {
                    window::remove_frame(window);
                    cx.new(|_| HmpGpuiApp)
                },
            )
            .expect("failed to open HMP GPUI desktop window");
            cx.activate(true);
        });
    ExitCode::SUCCESS
}
