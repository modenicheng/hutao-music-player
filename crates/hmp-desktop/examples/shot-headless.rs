//! 离屏 hover 截图宿主（合成器无关）：testing backend + software 渲染器，
//! mock 时间推进动画后 `Window::take_snapshot` 出图。
//! 用途：niri 焦点被空闲层抓走（dms fade-to-* exclusive grab）时验收 hover 动画；
//! 也用于确定性抓取滑块 mid-flight 帧（--flight MS）。
//!
//! 用法：
//!   cargo run --release -p hmp-desktop --example shot-headless -- library \
//!     --out /tmp/hmp-qa2/hl.png \
//!     [--move X,Y]        // 400ms 后合成 PointerMoved（真实 hover 链路）
//!     [--bus-hover X,Y,W,H[,suppress]]  // 直写 HoverBus（只验滑块几何）
//!     [--flight MS]       // 移动后再推进 MS 毫秒即截图（抓形变中帧；缺省推进到动画收敛）
//!
//! 注意：testing backend 是进程级单例，本示例进程内只有这一个窗口。

use std::sync::Arc;

use slint::{ComponentHandle, Global};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut route_str = String::from("library");
    let mut out = String::from("/tmp/hmp-qa2/hover-headless.png");
    let mut hover: Option<(f32, f32)> = None;
    let mut bus_hover: Option<(i32, f32, f32, f32, f32, bool)> = None;
    let mut then_hover: Option<(f32, f32)> = None;
    let mut pre_ms: u64 = 600;
    let mut flight: Option<u64> = None;
    let mut queue = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next().unwrap_or(out),
            "--hover" => {
                let v = args.next().unwrap_or_default();
                let (x, y) = v.split_once(',').expect("hover X,Y");
                hover = Some((x.parse().unwrap(), y.parse().unwrap()));
            }
            "--bus-hover" => {
                // GROUP,X,Y,W,H[,suppress]：GROUP = HoverGroup 的 id（init 序 1..N）
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
            "--then-hover" => {
                let v = args.next().unwrap_or_default();
                let (x, y) = v.split_once(',').expect("then-hover X,Y");
                then_hover = Some((x.parse().unwrap(), y.parse().unwrap()));
            }
            "--pre-ms" => pre_ms = args.next().and_then(|v| v.parse().ok()).unwrap_or(600),
            "--flight" => flight = args.next().and_then(|v| v.parse().ok()),
            "--queue" => queue = true,
            other => route_str = other.to_owned(),
        }
    }

    slint::platform::set_platform(Box::new(
        i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        ),
    ))
    .expect("platform already initialized");
    let prefs = std::sync::Arc::new(std::sync::Mutex::new(hmp_desktop::prefs::load()));
    let runtime = Arc::new(hmp_desktop::backend::BackendRuntime::new().expect("tokio runtime"));
    let ui = hmp_desktop::AppWindow::new()?;
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));

    hmp_desktop::bridge::load_data(&ui);
    hmp_desktop::bridge::bind(&ui, Arc::clone(&prefs), Arc::clone(&runtime));
    hmp_desktop::player_bridge::bind(&ui, runtime, prefs);

    {
        let nav = hmp_desktop::Nav::get(&ui);
        nav.invoke_go(
            parse_route(&route_str).expect("valid route"),
            "".into(),
        );
    }

    // 离线/无库环境下播种侧栏歌单，保证歌单行 hover 几何真实存在（同 hover_slider 测试）
    {
        let data = hmp_desktop::Data::get(&ui);
        let color = slint::Color::from_rgb_u8(0x1e, 0x38, 0x5f);
        data.set_sidebar_created(slint::ModelRc::new(slint::VecModel::from(vec![
            hmp_desktop::PlaylistCover { name: "测试歌单甲".into(), c1: color, c2: color },
            hmp_desktop::PlaylistCover { name: "测试歌单乙".into(), c1: color, c2: color },
        ])));
    }

    // 先推进一段 mock 时间，让初始过渡（侧栏宽度等）收敛，再注 hover；
    // 预热渲染一次：absolute-position 依赖布局求解，未渲染过就派发 hover 会让
    // HoverGroup 的包含性判定拿到陈旧组边界（块不亮）
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(800));

    if let Some((x, y)) = hover {
        ui.window().dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(x, y),
        });
    }
    if let Some((group, x, y, w, h, suppress)) = bus_hover {
        hmp_desktop::HoverBus::get(&ui).invoke_hover(
            group,
            slint::LogicalPosition::new(x, y),
            w,
            h,
            suppress,
        );
    }

    if queue {
        hmp_desktop::Player::get(&ui).set_queue_visible(true);
    }

    // 第二段 hover：先收敛第一次 placement，再移动触发行间滑动 + 黏滞形变
    if let Some((x, y)) = then_hover {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(pre_ms));
        ui.window().dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(x, y),
        });
    }

    // mock 时间必须分步推进：一次性大步进时 Slint 的属性动画（animate）插值
    // 不会走到目标值（渲染驱动器不在单次大跳中重评估），16ms 步进模拟帧
    let step_to = |total: u64| {
        let steps = (total + 15) / 16;
        for _ in 0..steps {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        }
    };


    match flight {
        // 抓动画中帧：只推进到指定时刻
        Some(ms) => step_to(ms),
        // 缺省推进到全部动画收敛（块 240ms 形变 + 淡入收尾）
        None => step_to(600),
    }





    let buffer = ui.window().take_snapshot()?;
    let (w, h) = (buffer.width(), buffer.height());
    let bytes = buffer.as_bytes();
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        let i = (y as usize * w as usize + x as usize) * 4;
        image::Rgba([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
    });
    img.save(&out)?;
    println!("saved {out} ({}x{})", w, h);
    Ok(())
}

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
