//! 视觉 QA 截图宿主（winit 真 UI）：命令行选路由 + 预置状态，供 grim 截图。
//! 用法：
//!   cargo run --release -p hmp-desktop --example shot -- library \
//!     [--param <mid>] [--playing] [--queue] [--hover X,Y] [--bus-hover X,Y,W,H[,suppress]] \
//!     [--theme light|dark]
//! --hover 用 Window::dispatch_event 合成 PointerMoved（触发真实 has-hover）；
//! --bus-hover 直接写 HoverBus（只验证滑块几何，不依赖指针遍历）。

use slint::{ComponentHandle, Global};

use hmp_desktop::{backend, bridge, player_bridge, prefs};

fn parse_route(s: &str) -> Option<hmp_desktop::Route> {
    use hmp_desktop::Route;
    Some(match s {
        "home" => Route::Home,
        "discover" => Route::Discover,
        "top" => Route::Top,
        "top-detail" => Route::TopDetail,
        "search" => Route::Search,
        "playlist" => Route::Playlist,
        "album" => Route::Album,
        "artist" => Route::Artist,
        "library" => Route::Library,
        "recent" => Route::Recent,
        "local" => Route::Local,
        "downloads" => Route::Downloads,
        "purchased" => Route::Purchased,
        "settings" => Route::Settings,
        "settings-general" => Route::SettingsGeneral,
        "settings-playback" => Route::SettingsPlayback,
        "settings-account" => Route::SettingsAccount,
        _ => return None,
    })
}

fn main() -> Result<(), slint::PlatformError> {
    let mut route = hmp_desktop::Route::Library;
    let mut param = String::new();
    let mut playing = false;
    let mut queue = false;
    let mut overlay = false;
    let mut wheel: Option<(f32, f32, f32, u32)> = None;
    let mut hover: Option<(f32, f32)> = None;
    let mut bus_hover: Option<(i32, f32, f32, f32, f32, bool)> = None;
    let mut theme: Option<i32> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--param" => param = args.next().unwrap_or_default(),
            "--playing" => playing = true,
            "--queue" => queue = true,
            "--overlay" => overlay = true,
            "--wheel" => {
                // X,Y,DY,N：位置 + 每次纵向增量 + 次数（滚动页面用）
                let v = args.next().unwrap_or_default();
                let parts: Vec<&str> = v.split(',').collect();
                wheel = Some((
                    parts[0].parse().unwrap(),
                    parts[1].parse().unwrap(),
                    parts[2].parse().unwrap(),
                    parts[3].parse().unwrap(),
                ));
            }
            "--hover" => {
                let v = args.next().unwrap_or_default();
                let (x, y) = v.split_once(',').expect("hover X,Y");
                hover = Some((x.parse().unwrap(), y.parse().unwrap()));
            }
            "--bus-hover" => {
                // GROUP,X,Y,W,H[,suppress]
                let v = args.next().unwrap_or_default();
                let parts: Vec<&str> = v.split(',').collect();
                bus_hover = Some((
                    parts[0].parse().unwrap(),
                    parts[1].parse().unwrap(),
                    parts[2].parse().unwrap(),
                    parts[3].parse().unwrap(),
                    parts[4].parse().unwrap(),
                    parts.get(5).map(|s| *s == "1").unwrap_or(false),
                ));
            }
            "--theme" => match args.next().unwrap_or_default().as_str() {
                "light" => theme = Some(1),
                "dark" => theme = Some(2),
                _ => {}
            },
            other => {
                if let Some(r) = parse_route(other) {
                    route = r;
                }
            }
        }
    }

    let prefs = std::sync::Arc::new(std::sync::Mutex::new(prefs::load()));
    let runtime = std::sync::Arc::new(backend::BackendRuntime::new().expect("tokio runtime"));
    let ui = hmp_desktop::AppWindow::new()?;

    if let Some(mode) = theme {
        hmp_desktop::Theme::get(&ui).set_mode(mode);
    }

    bridge::load_data(&ui);
    bridge::bind(
        &ui,
        std::sync::Arc::clone(&prefs),
        std::sync::Arc::clone(&runtime),
    );
    player_bridge::bind(
        &ui,
        std::sync::Arc::clone(&runtime),
        std::sync::Arc::clone(&prefs),
    );

    {
        let nav = hmp_desktop::Nav::get(&ui);
        nav.invoke_go(route, param.as_str().into());
    }

    if playing {
        // 走回调让播放桥下发 Play 请求（直接写 global 不会同步 daemon 状态）
        let player = hmp_desktop::Player::get(&ui);
        let liked = hmp_desktop::Data::get(&ui).get_liked();
        player.invoke_play_tracks(liked, 0);
    }
    if queue {
        hmp_desktop::Player::get(&ui).set_queue_visible(true);
    }
    if overlay {
        // 走回调（打开 + 触发评论装载），不直写属性
        hmp_desktop::Player::get(&ui).invoke_show_overlay();
    }

    let weak = ui.as_weak();
    if let Some((x, y, dy, times)) = wheel {
        use slint::platform::WindowEvent;
        for n in 0..times {
            let weak = weak.clone();
            slint::Timer::single_shot(
                std::time::Duration::from_millis(300 * (n as u64 + 1)),
                move || {
                    if let Some(ui) = weak.upgrade() {
                        for _ in 0..3 {
                            ui.window().dispatch_event(WindowEvent::PointerScrolled {
                                position: slint::LogicalPosition::new(x, y),
                                delta_x: 0.0,
                                delta_y: dy,
                            });
                        }
                    }
                },
            );
        }
    }
    if let Some((x, y)) = hover {
        slint::Timer::single_shot(std::time::Duration::from_millis(400), move || {
            use slint::platform::WindowEvent;
            if let Some(ui) = weak.upgrade() {
                ui.window().dispatch_event(WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(x, y),
                });
            }
        });
    }
    if let Some((group, x, y, w, h, suppress)) = bus_hover {
        let bus = hmp_desktop::HoverBus::get(&ui);
        bus.invoke_hover(group, slint::LogicalPosition::new(x, y), w, h, suppress);
    }

    ui.run()
}
