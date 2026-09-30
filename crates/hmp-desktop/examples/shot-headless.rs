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

fn seed_demo(ui: &hmp_desktop::AppWindow) {
    use hmp_desktop::{CommentRow, LyricRow, NowPlaying, Player};
    let player = Player::get(ui);
    player.set_has_track(true);
    player.set_title("希望有羽毛和翅膀".into());
    player.set_artists("知更鸟 / HOYO-MiX / Chevy".into());
    player.set_album("崩坏星穹铁道-空气蛹 INSIDE".into());
    player.set_playing(true);
    player.set_duration_ms(229_000);
    player.set_position_ms(61_000);
    player.set_progress(61.0 / 229.0);
    // 封面 + 曲目层取色/环境层模糊底图（走真实管线，视觉验收含取色效果）
    let cover = hmp_desktop::covers::cover_image("album:overlay-demo");
    player.set_cover(cover.clone());
    hmp_desktop::track_theme::apply_cover(&ui.as_weak(), "seed:overlay-demo", &cover);

    // 行级 LRC 样例（时间轴覆盖 60s 附近，验证焦点行与弹簧）。每第 4 行
    // 塞长句（窄列必折行，验收行盒自适应不裁字）、奇数行带翻译（验收翻译
    // 行排版与行高累加）
    let lrc = (0..24)
        .map(|i| {
            let t = 8_000 + i as u64 * 6_500;
            let m = t / 60_000;
            let sec = (t % 60_000) / 1_000;
            let body = if i % 4 == 2 {
                "歌词第%行 希望有羽毛和翅膀 这是刻意加长的句子用来验收长歌词行折行排版不被裁切且行高按内容自适应".replacen('%', &format!("{}", i % 10 + 1), 1)
            } else {
                format!("歌词第{}行 希望有羽毛和翅膀", i % 10 + 1)
            };
            format!("[{m:02}:{sec:02}.{:02}]{body}", (t % 1000) / 10)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let trans_lrc = (0..24)
        .filter(|i| i % 2 == 1)
        .map(|i| {
            let t = 8_000 + i as u64 * 6_500;
            let m = t / 60_000;
            let sec = (t % 60_000) / 1_000;
            format!(
                "[{m:02}:{sec:02}.{:02}]Wish I had feathers and wings (line {})",
                (t % 1000) / 10,
                i % 10 + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let lines = hmp_desktop::parse_lrc(&lrc, &trans_lrc);
    let rows: Vec<LyricRow> = lines
        .iter()
        .map(|l| LyricRow {
            timestamp_ms: l.timestamp_ms as i32,
            text: l.text.clone().into(),
            translation: l.translation.clone().into(),
        })
        .collect();
    let np = NowPlaying::get(ui);
    np.set_lyrics(slint::ModelRc::new(slint::VecModel::from(rows)));
    np.set_lyrics_generation(1);
    // 61s → 焦点行 8（8s 起每 6.5s 一行），模拟 Rust 推送折算结果
    np.set_active_line(8);

    let comments: Vec<CommentRow> = (0..6)
        .map(|i| CommentRow {
            cm_id: format!("cm{i}").into(),
            nickname: format!("乐评人{i}").into(),
            initial: "乐".into(),
            content: format!(
                "第{i}条评论：这段旋律把星穹列车的旅途感写尽了，知更鸟的声线像羽毛一样落在心上。"
            )
            .into(),
            time_text: "09-08 14:0".into(),
            like_text: if i == 0 {
                "2.3万".into()
            } else {
                format!("{}", 1000 - i * 137).into()
            },
        })
        .collect();
    np.set_comments(slint::ModelRc::new(slint::VecModel::from(comments)));
    np.set_comment_total("8.6万".into());
    np.set_comment_state(2);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut route_str = String::from("library");
    let mut out = String::from("/tmp/hmp-qa2/hover-headless.png");
    let mut param = String::new();
    let mut hover: Option<(f32, f32)> = None;
    let mut bus_hover: Option<(i32, f32, f32, f32, f32, bool)> = None;
    let mut then_hover: Option<(f32, f32)> = None;
    let mut pre_ms: u64 = 600;
    let mut flight: Option<u64> = None;
    let mut queue = false;
    let mut overlay = false;
    let mut account_demo = 0i32;
    let mut wheel: Option<(f32, f32, f32, u32)> = None;
    let mut wait_ms: u64 = 0;
    let mut seed = false;
    let mut discover_live = false;
    let mut loop_mode = 0i32;
    let mut shuffle = false;
    let mut theme_dark = false;

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
            "--overlay" => overlay = true,
            // 合成数据灌 Player/NowPlaying（布局/弹簧/高亮验收；不依赖 daemon 推送）
            "--seed" => seed = true,
            // 账号页演示态（合成数据；daemon 真实态在沙箱不可确定性呈现）：
            // 1=已登录 2=扫码中（QR 面板展开）3=未登录（扫码入口可见）
            "--account-demo" => {
                account_demo = args.next().and_then(|v| v.parse().ok()).unwrap_or(0)
            }
            "--theme" => {
                theme_dark = args.next().map(|v| v == "dark").unwrap_or(false);
            }
            // 播放模式态（验收 循环三态/随机 高亮；0=顺序 1=单曲 2=列表）
            "--loop" => loop_mode = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--shuffle" => shuffle = true,
            // 真实时间等待 IPC/网络回包（回调进 slint 队列，随后续 step_to 处理）
            "--wait" => wait_ms = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            // X,Y,DY：截图前在 (X,Y) 派发 N 次滚轮（滚动到评论区用）
            "--wheel" => {
                let v = args.next().unwrap_or_default();
                let parts: Vec<&str> = v.split(',').collect();
                wheel = Some((
                    parts[0].parse().unwrap(),
                    parts[1].parse().unwrap(),
                    parts[2].parse().unwrap(),
                    parts[3].parse().unwrap(),
                ));
            }
            "--param" => param = args.next().unwrap_or_default(),
            "--discover-live" => discover_live = true,
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
            param.clone().into(),
        );
    }
    // 暗色验收：直接设档（不走 set-mode 回调，免落 prefs）
    if theme_dark {
        hmp_desktop::Theme::get(&ui).set_mode(2);
    }

    // 离线/无库环境下播种侧栏歌单，保证歌单行 hover 几何真实存在（同 hover_slider 测试）
    {
        let data = hmp_desktop::Data::get(&ui);
        data.set_sidebar_created(slint::ModelRc::new(slint::VecModel::from(vec![
            hmp_desktop::PlaylistCover {
                id: "1".into(),
                name: "测试歌单甲".into(),
                image: hmp_desktop::covers::cover_image("playlist:1"),
            },
            hmp_desktop::PlaylistCover {
                id: "2".into(),
                name: "测试歌单乙".into(),
                image: hmp_desktop::covers::cover_image("playlist:2"),
            },
        ])));
    }

    // —— 发现页真实数据诊断模式（--discover-live）———
    // 测试后端下 invoke_from_event_loop 的回调在 mock 泵里不出队（异步回包
    // 饿死，真实 GUI 由 winit 事件循环驱动无此限制），在线内容页在无头截图
    // 里永远停在加载文案。此模式在 UI 线程同步 block_on 拉真实发现页 +
    // 逐卡 CoverGet 换真图（与生产同一 daemon IPC 链路），直接落模型——
    // 专供「真实封面渲染」验收；异步竞态防护仍以真实 GUI 为准。
    if discover_live {
        let data = hmp_desktop::Data::get(&ui);
        let block_rt = tokio::runtime::Runtime::new().expect("block-on runtime");
        let page = match block_rt.block_on(hmp_desktop::backend::request(
            hmp_core::Request::DiscoverGet {
                songlist_page: 1,
                new_song_type: 5,
            },
        )) {
            Ok(hmp_core::Response::Discover(page)) => page,
            other => panic!("discover-live: daemon request failed: {other:?}"),
        };
        let mut cards: Vec<hmp_desktop::CoverCardData> = page
            .playlists
            .iter()
            .map(|p| hmp_desktop::CoverCardData {
                mid: p.id.to_string().into(),
                title: p.title.as_str().into(),
                subtitle: format!("{} 首 · {}", p.songnum, p.creator).into(),
                cover: hmp_desktop::covers::cover_image(&format!("playlist:{}", p.id)),
            })
            .collect();
        data.set_discover_new_songs(slint::ModelRc::new(slint::VecModel::from(
            page.new_songs.iter().map(discover_row).collect::<Vec<_>>(),
        )));
        data.set_discover_state(2);
        // 逐卡取真图（同步；上限 12 张控制截图时长），先占位后替换
        for (i, p) in page.playlists.iter().enumerate().take(12) {
            if p.picurl.is_empty() {
                continue;
            }
            if let Ok(hmp_core::Response::Cover(uri)) =
                block_rt.block_on(hmp_desktop::backend::request(hmp_core::Request::CoverGet {
                    url: p.picurl.clone(),
                }))
            {
                let path = uri.strip_prefix("file://").unwrap_or(&uri);
                if let Ok(image) = slint::Image::load_from_path(std::path::Path::new(path)) {
                    if let Some(card) = cards.get_mut(i) {
                        card.cover = image;
                    }
                }
            }
        }
        data.set_discover_playlists(slint::ModelRc::new(slint::VecModel::from(cards)));
    }

    // 账号页演示态（仅截图配方；1=已登录 2=扫码中）。
    // bind 的真实 AccountStatus 查询在沙箱 daemon 不可达，状态会漂——演示
    // 值在 wait 之后最终覆写一次，保证截图确定性。
    if account_demo > 0 {
        let data = hmp_desktop::Data::get(&ui);
        data.set_account_state(1);
        if account_demo == 1 {
            data.set_account_logged_in(true);
            data.set_account_nickname("胡桃".into());
            data.set_account_uin("10001".into());
            data.set_account_vip("VIP 会员".into());
        } else if account_demo == 2 {
            // 扫码中：程序化 QR 占位图（黑白格，仅验证面板布局与对比度）
            data.set_login_state(1);
            data.set_login_qr(qr_placeholder_image());
        }
        // account_demo == 3：未登录（state=1 + logged_in=false），扫码入口可见
    }

    // 先推进一段 mock 时间，让初始过渡（侧栏宽度等）收敛，再注 hover；
    // 预热渲染一次：absolute-position 依赖布局求解，未渲染过就派发 hover 会让
    // HoverGroup 的包含性判定拿到陈旧组边界（块不亮）。推进必须像下方 step_to
    // 一样分步：单次大步进后合成指针的 has-hover 永不触发（hover 探针二分：
    // 50×16ms 分步的进程内序列有效，一次 800ms 跳进后单动无效）
    for _ in 0..50 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
    if hover.is_some() || then_hover.is_some() || bus_hover.is_some() {
        let warm = ui.window().take_snapshot()?;
        drop(warm);
    }

    if let Some((x, y)) = hover {
        // 进程内首个 PointerMoved 会被吞（testing backend 鼠标状态未初始化；
        // hover 探针二分确证：单动无效、序列中的后续动有效）。真实指针 arrivals
        // 都带移动轨迹，这里先派发一次中性位（播放条，无 HoverItem）热身移动。
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(640.0, 760.0),
            });
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(32));
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
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
    if seed {
        seed_demo(&ui);
    }
    if loop_mode != 0 || shuffle {
        let player = hmp_desktop::Player::get(&ui);
        player.set_loop_mode(loop_mode);
        player.set_shuffle(shuffle);
    }
    if overlay {
        // 走回调（打开 overlay + 触发评论装载），与真实点击链路一致
        hmp_desktop::Player::get(&ui).invoke_show_overlay();
    }

    // 第二段 hover：先收敛第一次 placement，再移动触发行间滑动 + 黏滞形变
    if let Some((x, y)) = then_hover {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(pre_ms));
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(x, y),
            });
    }

    // mock 时间必须分步推进：一次性大步进时 Slint 的属性动画（animate）插值
    // 不会走到目标值（渲染驱动器不在单次大跳中重评估），16ms 步进模拟帧
    let step_to = |total: u64| {
        let steps = total.div_ceil(16);
        for _ in 0..steps {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        }
    };

    if let Some((x, y, dy, times)) = wheel {
        for _ in 0..times {
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(x, y),
                    delta_x: 0.0,
                    delta_y: dy,
                });
            step_to(80);
        }
    }

    if wait_ms > 0 {
        // 真实时间等 IPC/网络，同时小步泵帧——invoke_from_event_loop 的回调
        // 只在事件循环迭代里执行，纯 sleep 会把回包堵在队列外
        let cycles = wait_ms / 100;
        for _ in 0..cycles {
            std::thread::sleep(std::time::Duration::from_millis(100));
            step_to(100);
        }
    }

    // 账号页演示值截图前最终覆写：bind 的真实 AccountStatus 重试/回包若在
    // 上面的泵帧窗口内落地，会把演示态冲掉——这里再写一次保证确定性。
    if account_demo > 0 {
        let data = hmp_desktop::Data::get(&ui);
        data.set_account_state(1);
        if account_demo == 1 {
            data.set_account_logged_in(true);
            data.set_account_nickname("胡桃".into());
            data.set_account_uin("10001".into());
            data.set_account_vip("VIP 会员".into());
        } else if account_demo == 2 {
            data.set_login_state(1);
            data.set_login_qr(qr_placeholder_image());
            data.set_login_message(String::new().into());
        }
        // account_demo == 3：未登录（state=1 + logged_in=false），扫码入口可见
    }

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

/// DiscoverNewSong → TrackRow（--discover-live 专用；与 bridge 同投影，
/// 封面程序化占位——列表行不做逐行网络取图）。
fn discover_row(s: &hmp_core::DiscoverNewSong) -> hmp_desktop::TrackRow {
    hmp_desktop::TrackRow {
        mid: s.mid.as_str().into(),
        source: 0,
        title: s.name.as_str().into(),
        artists: s.singer.as_str().into(),
        artist_mid: "".into(),
        album: s.album.as_str().into(),
        album_mid: "".into(),
        duration_ms: (s.interval * 1000) as i32,
        quality: "".into(),
        cover: hmp_desktop::covers::cover_image(&format!("album:{}", s.mid)),
    }
}

/// 程序化 QR 占位图（33×33 黑白格 + 定位角；仅账号页扫码面板布局验收）。
fn qr_placeholder_image() -> slint::Image {
    const N: usize = 33;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(N as u32, N as u32);
    let pixels = buf.make_mut_slice();
    let finder =
        |r: usize, c: usize| (r < 7 && c < 7) || (r < 7 && c >= N - 7) || (r >= N - 7 && c < 7);
    for r in 0..N {
        for c in 0..N {
            let dark = finder(r, c)
                || ((r * 7 + c * 13 + ((r / 3) * 5)) ^ (c / 2)) % 3 == 0 && !finder(r, c);
            pixels[r * N + c] = if dark {
                slint::Rgba8Pixel::new(0x10, 0x10, 0x12, 0xFF)
            } else {
                slint::Rgba8Pixel::new(0xFF, 0xFF, 0xFF, 0xFF)
            };
        }
    }
    slint::Image::from_rgba8(buf)
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
