//! HMP GPUI desktop prototype bootstrap.
//!
//! The window setup is adapted from `cradiy/gpui-apple-music-demo` (MIT),
//! while playback remains owned by HMP Core.

use std::process::ExitCode;

#[cfg(target_os = "macos")]
use gpui::point;
use gpui::{
    App, AppContext, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowOptions, px, size,
};
use gpui_platform::application;
use uic::assets::LucideAssets;

mod app;
mod components;
mod state;
mod theme;
mod window;

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
                    cx.new(|cx| app::HmpGpuiApp::new(cx, window))
                },
            )
            .expect("failed to open HMP GPUI desktop window");
            cx.activate(true);
        });
    ExitCode::SUCCESS
}
