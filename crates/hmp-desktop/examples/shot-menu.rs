//! S6 验收示例（QA 专用，非产品代码）：右键菜单离屏截图宿主 + §21 窗口化浏览模拟。
//!
//! 模式一（默认，菜单截图）：testing backend + software renderer，播种合成数据，
//! 右键（PointerPressed/Released Right，tests/context_menu.rs 同款手法）打开
//! TrackMenuPanel 弹层，可选菜项 hover / 菜项点击（进 picker 二级面板），
//! take_snapshot 落 PNG。弹层经 WindowInner::draw_contents 的 ChildWindow 路径
//! 进快照（slint 1.17.1 vendored 源码确证；软件渲染器「三不渲染」：阴影不画、
//! clip 圆角忽略、纹理最近邻——均为已知项，不判缺陷）。
//!
//! 模式二（--browse-loop，内存代理）：testing backend 下 Scroll→Viewport 回写
//! 不生效、view-h 恒 0，TrackTable 退化为全量渲染（tests/context_menu.rs:10-12
//! 实证）——只滚 Flickular 测到的是旧全量路径。故从 Rust 侧**直驱导出的
//! Viewport 全局**（set_view_h + 步进 set_y 遍历模型全程）切进窗口化虚拟化
//! 路径，并以「深滚屏位右键无命中 / 回顶同位命中」判别法设早期门：全量路径
//! 下 window-start 恒 0、行恒在首屏，深滚后屏位仍命中——两模式可判别。
//! 外部进程按 PID 采样本进程 RSS 作 §21 根因 3（行不背图）/根因 4（窗口化）
//! /模型重建的代理指标（软件渲染器、无 GPU 纹理、无 daemon 推送）。
//!
//! 用法（菜单截图）：
//!   cargo run --release -p hmp-desktop --example shot-menu -- library \
//!     --out <png> [--theme dark] [--right X,Y] [--menu-hover N | --menu-click N]
//!     [--queue] [--playlist-param ID] [--no-seed-playlists] [--nav-mids]
//!     [--rows-source qq|local] [--rows N] [--wait-ms 2500]
//! 用法（浏览代理）：
//!   cargo run --release -p hmp-desktop --example shot-menu -- library \
//!     --browse-loop --browse-minutes 6 [--rows 4500]
//!
//! 注意：testing backend 进程级单例；本示例经 backend::connect_or_spawn 可能拉起
//! daemon——运行方必须以隔离环境（XDG_*_HOME + HMP_IPC_ENDPOINT，见
//! verify/scripts/env-s6.ps1）包裹，否则会触碰默认管道与用户真库。

use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::platform::PointerEventButton;
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Global, LogicalPosition};

// 曲目行探测基准（tests/context_menu.rs 同源：library 首行顶≈293、行距 41，
// y=350 落第 2 行中部；此处仅作扫描起点，命中以 TrackMenu.track 哨兵判别为准）
const PROBE_X: f32 = 640.0;

fn step16(n: u64) {
    for _ in 0..n {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(16));
    }
}

fn settle() {
    step16(40);
}

fn move_to(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(x, y),
    });
}

fn press(ui: &hmp_desktop::AppWindow, x: f32, y: f32, button: PointerEventButton) {
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: LogicalPosition::new(x, y),
        button,
    });
}

fn release(ui: &hmp_desktop::AppWindow, x: f32, y: f32, button: PointerEventButton) {
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(x, y),
        button,
    });
}

fn right_click(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    press(ui, x, y, PointerEventButton::Right);
    release(ui, x, y, PointerEventButton::Right);
}

fn left_click(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    press(ui, x, y, PointerEventButton::Left);
    release(ui, x, y, PointerEventButton::Left);
}

fn seed_track_rows(count: i32, nav_mids: bool, source: i32) -> Vec<hmp_desktop::TrackRow> {
    (0..count)
        .map(|i| hmp_desktop::TrackRow {
            mid: format!("menu-track-{i}").into(),
            source,
            title: format!("曲目 {i} · 胡桃木验收").into(),
            artists: "测试歌手 / 副歌手".into(),
            artist_mid: if nav_mids {
                format!("歌手{i}").into()
            } else {
                String::new().into()
            },
            album: "测试专辑".into(),
            album_mid: if nav_mids {
                format!("专辑{i}").into()
            } else {
                String::new().into()
            },
            duration_ms: 200_000 + i * 1000,
            quality: if i % 3 == 0 {
                "FLAC · 44.1kHz".into()
            } else {
                "".into()
            },
        })
        .collect()
}

fn seed_queue_rows(count: i32) -> Vec<hmp_desktop::QueueRow> {
    (0..count)
        .map(|i| hmp_desktop::QueueRow {
            mid: format!("queue-row-{i}").into(),
            source: 0,
            title: format!("队列曲 {i}").into(),
            artists: "队列歌手".into(),
            artist_mid: "".into(),
            album: "队列专辑".into(),
            album_mid: "".into(),
            duration_ms: 180_000,
            quality: "".into(),
            cover: hmp_desktop::covers::cover_image(&format!("queue:{i}")),
        })
        .collect()
}

fn seed_sidebar(ui: &hmp_desktop::AppWindow) {
    let data = hmp_desktop::Data::get(ui);
    data.set_sidebar_created(slint::ModelRc::new(slint::VecModel::from(vec![
        hmp_desktop::PlaylistCover {
            id: "1".into(),
            name: "测试歌单甲".into(),
            relation: "local".into(),
            image: hmp_desktop::covers::cover_image("playlist:1"),
        },
        hmp_desktop::PlaylistCover {
            id: "2".into(),
            name: "测试歌单乙".into(),
            relation: "owned".into(),
            image: hmp_desktop::covers::cover_image("playlist:2"),
        },
    ])));
}

/// 在 (x, ys) 候选序列上右键探测命中行：以 TrackMenu.track.mid 哨兵判别
/// （index 在弹层关闭后不复位，不可作哨兵）。命中返回命中点。
fn probe_row(
    ui: &hmp_desktop::AppWindow,
    x: f32,
    ys: impl Iterator<Item = f32>,
) -> Option<(f32, f32)> {
    let menu = hmp_desktop::TrackMenu::get(ui);
    for y in ys {
        let sentinel = SharedString::from(format!("probe-sentinel-{y}"));
        let mut t = menu.get_track();
        t.mid = sentinel.clone();
        menu.set_track(t);
        right_click(ui, x, y);
        settle();
        if menu.get_track().mid != sentinel {
            return Some((x, y));
        }
    }
    None
}

use slint::SharedString;

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

fn save_snapshot(ui: &hmp_desktop::AppWindow, out: &str) -> Result<(), Box<dyn std::error::Error>> {
    let buffer = ui.window().take_snapshot()?;
    let (w, h) = (buffer.width(), buffer.height());
    let bytes = buffer.as_bytes();
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        let i = (y as usize * w as usize + x as usize) * 4;
        image::Rgba([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
    });
    img.save(out)?;
    println!("saved {out} ({w}x{h})");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut route_str = String::from("library");
    let mut out = String::from("verify/shots/menu/shot.png");
    let mut param = String::new();
    let mut theme_dark = false;
    let mut right: Option<(f32, f32)> = None;
    let mut menu_hover: Option<i32> = None;
    let mut menu_click: Option<i32> = None;
    // 调试旗标：拆解 press/release 对弹层存活的影响（testing backend 输入管线排查）
    let mut click_press_only = false;
    let mut click_release_only = false;
    let mut queue_mode = false;
    let mut playlist_param: Option<String> = None;
    let mut seed_playlists = true;
    let mut nav_mids = false;
    let mut rows_source: i32 = 0;
    let mut rows: i32 = 30;
    let mut wait_ms: u64 = 2500;
    let mut browse_loop = false;
    let mut browse_minutes: u64 = 6;
    let mut browse_force = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next().unwrap_or(out),
            "--param" => param = args.next().unwrap_or_default(),
            "--theme" => theme_dark = args.next().map(|v| v == "dark").unwrap_or(false),
            "--right" => {
                let v = args.next().unwrap_or_default();
                let (x, y) = v.split_once(',').expect("--right X,Y");
                right = Some((x.parse().unwrap(), y.parse().unwrap()));
            }
            "--menu-hover" => menu_hover = args.next().and_then(|v| v.parse().ok()),
            "--menu-click" => menu_click = args.next().and_then(|v| v.parse().ok()),
            "--click-press-only" => click_press_only = true,
            "--click-release-only" => click_release_only = true,
            "--queue" => queue_mode = true,
            "--playlist-param" => playlist_param = Some(args.next().unwrap_or_default()),
            "--no-seed-playlists" => seed_playlists = false,
            "--nav-mids" => nav_mids = true,
            "--rows-source" => {
                rows_source = match args.next().as_deref() {
                    Some("local") => 1,
                    _ => 0,
                }
            }
            "--rows" | "--seed-rows" => {
                rows = args.next().and_then(|v| v.parse().ok()).unwrap_or(30)
            }
            "--wait-ms" => wait_ms = args.next().and_then(|v| v.parse().ok()).unwrap_or(2500),
            "--browse-loop" => browse_loop = true,
            "--browse-minutes" => {
                browse_minutes = args.next().and_then(|v| v.parse().ok()).unwrap_or(6)
            }
            "--browse-force" => browse_force = true,
            other => route_str = other.to_owned(),
        }
    }

    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .expect("platform already initialized");

    let prefs = Arc::new(std::sync::Mutex::new(hmp_desktop::prefs::load()));
    let runtime = Arc::new(hmp_desktop::backend::BackendRuntime::new().expect("tokio runtime"));
    let ui = hmp_desktop::AppWindow::new()?;
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));

    hmp_desktop::bridge::load_data(&ui, &runtime);
    hmp_desktop::bridge::bind(&ui, Arc::clone(&prefs), Arc::clone(&runtime));
    hmp_desktop::player_bridge::bind(&ui, Arc::clone(&runtime), Arc::clone(&prefs));
    hmp_desktop::menu_bridge::bind(&ui, prefs, runtime);

    {
        let nav = hmp_desktop::Nav::get(&ui);
        nav.invoke_go(
            parse_route(&route_str).expect("valid route"),
            param.clone().into(),
        );
    }
    if theme_dark {
        hmp_desktop::Theme::get(&ui).set_mode(2);
    }

    // —— 浏览代理模式（§21 窗口化直驱 + 判别法早期门）——
    if browse_loop {
        browse_loop_main(&ui, rows, browse_minutes, browse_force)?;
        return Ok(());
    }

    // —— 菜单截图模式 ——
    // 先真实时间等 daemon 初始快照落完（隔离环境下 connect_or_spawn 会拉起隔离
    // daemon；空库快照若在播种后才到，会把播种模型冲掉——shot-headless --wait 同款）
    {
        let cycles = wait_ms / 100;
        for _ in 0..cycles {
            std::thread::sleep(Duration::from_millis(100));
            step16(6);
        }
    }

    // 播种（在 daemon 快照之后，防冲掉）
    {
        let data = hmp_desktop::Data::get(ui);
        let tracks = seed_track_rows(rows, nav_mids, rows_source);
        data.set_liked(slint::ModelRc::new(slint::VecModel::from(tracks)));
        data.set_liked_count(rows);
        if playlist_param.is_some() {
            let ptracks = seed_track_rows(rows.min(24), nav_mids, rows_source);
            data.set_playlist_found(true);
            data.set_playlist_name("歌单验收·胡桃木".into());
            data.set_playlist_tracks(slint::ModelRc::new(slint::VecModel::from(ptracks)));
        }
        if seed_playlists {
            seed_sidebar(&ui);
        }
    }
    if queue_mode {
        let player = hmp_desktop::Player::get(&ui);
        player.set_queue(slint::ModelRc::new(slint::VecModel::from(seed_queue_rows(
            40,
        ))));
        player.set_queue_visible(true);
    }

    settle();

    // 热身：中性位 move（进程内首个 PointerMoved 会被吞）+ 预热渲染一次
    //（absolute-position 依赖布局求解，未渲染过就派发事件会拿到陈旧几何）
    move_to(&ui, 640.0, 760.0);
    step16(2);
    {
        let warm = ui.window().take_snapshot()?;
        drop(warm);
    }

    // 定位右键点：显式 --right 优先；否则探测（曲目表自文档坐标起，队列抽屉
    // 在右缘列扫描；探测以 TrackMenu.track 哨兵判别，命中即打印）
    let (cx, cy) = match right {
        Some(p) => p,
        None => {
            let found = if queue_mode {
                probe_row(&ui, 1090.0, (140..=740).step_by(20).map(|v| v as f32))
            } else if playlist_param.is_some() {
                probe_row(&ui, PROBE_X, (200..=740).step_by(20).map(|v| v as f32))
            } else {
                probe_row(&ui, PROBE_X, (300..=740).step_by(20).map(|v| v as f32))
            };
            match found {
                Some(p) => {
                    println!("probe hit at {p:?}");
                    p
                }
                None => {
                    eprintln!("PROBE MISS: 未命中任何可右键行（播种/路由异常）");
                    save_snapshot(&ui, &out)?;
                    std::process::exit(2);
                }
            }
        }
    };

    // 行 hover + 右键打开菜单（释放触发）
    move_to(&ui, cx, cy);
    step16(30);
    right_click(&ui, cx, cy);
    settle();
    let menu = hmp_desktop::TrackMenu::get(&ui);
    println!(
        "menu: context={:?} index={} mid={} open=({:?},{:?})",
        menu.get_context(),
        menu.get_index(),
        menu.get_track().mid,
        menu.get_open_x(),
        menu.get_open_y()
    );

    // 菜单项 hover / 点击（面板 padding 8 + 行高 36 + 行距 2 → 第 N 项中心 =
    // popup_y + 8 + N*38 + 18；popup 顶 ≈ 点击点 y+4，x ≈ 点击点 x，钳制时近似仍够用）
    let panel_x = cx + 108.0;
    if let Some(n) = menu_hover {
        let item_y = cy + 4.0 + 8.0 + n as f32 * 38.0 + 18.0;
        move_to(&ui, panel_x, item_y);
        step16(30);
        println!("menu-hover item {n} at ({panel_x},{item_y})");
    }
    if let Some(n) = menu_click {
        let item_y = cy + 4.0 + 8.0 + n as f32 * 38.0 + 18.0;
        move_to(&ui, panel_x, item_y);
        step16(6);
        if !click_release_only {
            press(&ui, panel_x, item_y, PointerEventButton::Left);
        }
        step16(6);
        if !click_press_only {
            release(&ui, panel_x, item_y, PointerEventButton::Left);
        }
        step16(30);
        println!(
            "menu-click item {n} at ({panel_x},{item_y}) press_only={click_press_only} release_only={click_release_only}"
        );
    }

    step16(20);
    save_snapshot(&ui, &out)?;
    Ok(())
}

/// 浏览代理主循环：窗口化直驱 + 判别法早期门 + 全程滚动 × 路由循环。
fn browse_loop_main(
    ui: &hmp_desktop::AppWindow,
    rows: i32,
    minutes: u64,
    browse_force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let data = hmp_desktop::Data::get(ui);
    let reseed = || {
        let tracks = seed_track_rows(rows, false, 0);
        data.set_liked(slint::ModelRc::new(slint::VecModel::from(tracks)));
        data.set_liked_count(rows);
    };
    reseed();
    seed_sidebar(ui);
    {
        let d = hmp_desktop::Data::get(ui);
        let ptracks = seed_track_rows(600, false, 0);
        d.set_playlist_found(true);
        d.set_playlist_name("浏览代理歌单".into());
        d.set_playlist_tracks(slint::ModelRc::new(slint::VecModel::from(ptracks)));
    }

    settle();
    // 初始为全量渲染（view-h=0，testing backend 特性）——这一拍是「旧路径」基线，
    // 允许一次性 RSS 跳变；随后直驱 Viewport 切进窗口化。
    {
        let warm = ui.window().take_snapshot()?;
        drop(warm);
        println!("browse: seeded {rows} rows, initial full-render warm done");
    }

    // —— 早期门（判别法）：窗口化生效 = 深滚屏位 MISS / 回顶同位 HIT ——
    let vp = hmp_desktop::Viewport::get(ui);
    let pitch: f32 = 41.0;
    let view_h: f32 = 631.0;
    vp.set_view_h(view_h);
    vp.set_y(0.0);
    step16(20);
    let menu = hmp_desktop::TrackMenu::get(ui);

    let sentinel_probe = |tag: &str| -> bool {
        let sentinel = SharedString::from(format!("gate-{tag}"));
        let mut t = menu.get_track();
        t.mid = sentinel.clone();
        menu.set_track(t);
        right_click(ui, PROBE_X, 350.0);
        settle();
        let hit = menu.get_track().mid != sentinel;
        println!(
            "gate {tag}: hit={hit} mid={} (view-h={} y={})",
            menu.get_track().mid,
            vp.get_view_h(),
            vp.get_y()
        );
        hit
    };

    let top_hit = sentinel_probe("top"); // 回顶：应命中（窗口 0..31 含首屏行）
    // 关闭可能打开的菜单（点外）：中性左键
    left_click(ui, 640.0, 30.0);
    settle();

    // 关菜单（popup 关闭）可能触发重布局 → Scroll 的 init/changed 回写可能把
    // view-h 打回 0；每次探测前重申窗口化参数（幂等）。
    let deep_y = -((rows as f32 - 20.0) * pitch);
    vp.set_view_h(view_h);
    vp.set_y(deep_y);
    step16(20);
    let deep_hit = sentinel_probe("deep"); // 深滚：应 MISS（窗口行在内容深处）

    left_click(ui, 640.0, 30.0);
    settle();
    vp.set_view_h(view_h);
    vp.set_y(0.0);
    step16(20);
    let top_hit2 = sentinel_probe("top2"); // 回顶复原：应再次命中

    let windowed = top_hit && !deep_hit && top_hit2;
    println!("GATE: windowed={windowed} (期望 true；false=全量退化)");
    if !windowed && !browse_force {
        eprintln!(
            "GATE FAIL: Viewport 直驱未切进窗口化路径，浏览代理数据无效（--browse-force 可在全量模式下继续，只覆盖根因 3+模型重建）"
        );
        std::process::exit(3);
    }
    if !windowed {
        eprintln!(
            "GATE FAIL 但 --browse-force：继续全量渲染模式（只覆盖根因 3 行不背图 + 模型重建，不覆盖窗口化）"
        );
    }
    // 收起菜单，进入主循环
    left_click(ui, 640.0, 30.0);
    settle();

    let started = Instant::now();
    let deadline = started + Duration::from_secs(minutes * 60);
    let routes = [
        ("library", ""),
        ("recent", ""),
        ("local", ""),
        ("playlist", "1"),
        ("album", "测试专辑"),
        ("artist", "测试歌手"),
        ("search", ""),
        ("settings", ""),
    ];
    let max_scroll = -(rows as f32 - 16.0) * pitch;
    let mut pass: u64 = 0;
    while Instant::now() < deadline {
        for (r, p) in routes {
            if Instant::now() >= deadline {
                break;
            }
            let nav = hmp_desktop::Nav::get(ui);
            nav.invoke_go(parse_route(r).expect("route"), p.into());
            step16(8);
            // 路由切换可能触发 Scroll init 回写 → 重申窗口化参数（幂等）
            vp.set_view_h(view_h);
            // 全程滚动：步进 view_h/2，从顶滚到底再瞬回
            let mut y = 0.0f32;
            while y > max_scroll {
                vp.set_view_h(view_h);
                vp.set_y(y);
                step16(4);
                std::thread::sleep(Duration::from_millis(1));
                y -= view_h / 2.0;
                if Instant::now() >= deadline {
                    break;
                }
            }
            vp.set_y(0.0);
            step16(4);
        }
        reseed(); // 模型重建 churn（每次全量重灌 4500 行）
        pass += 1;
        println!(
            "browse pass {pass} done, elapsed {}s",
            started.elapsed().as_secs()
        );
    }
    println!("browse loop finished after {pass} passes");
    Ok(())
}
