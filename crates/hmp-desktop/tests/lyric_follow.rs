//! 歌词进页直达当前行（testing backend，单测试单进程；软件渲染器取像素）。
//!
//! 回归护栏（Slint 1.19 升级实测）：歌词行靠 `changed y / height / is-active`
//! 向跟随逻辑上报自己的位置。播放页在「当前行已定」时才挂载的话，行带着最终
//! 几何创建，这三个回调一次都不触发，页面停在词表顶部，要等下一句开始才跟
//! 上（1.18 里行先落 0 再被布局赋值，碰巧会触发）。修法是舞台在等待上报期间
//! 每拍向各行要一次（`report-probe`）。
//!
//! 判据：快照的歌词视口里，强调色（当前句）像素的质心应落在视口中部——修复
//! 前当前句在视口之外，视口里没有任何强调色像素。
//!
//! 出网隔离：HMP_IPC_ENDPOINT 指向不存在的 socket；歌词数据由本测试播种。

use std::sync::Arc;

use slint::{ComponentHandle, Global, Rgba8Pixel, SharedPixelBuffer};

use hmp_desktop::{LyricRow, NowPlaying, Player};

fn settle(ms: u32) {
    for _ in 0..(ms / 16).max(1) {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

/// 歌词视口内强调色文字像素的质心 y（1280×800：视口约 y 39–696）。
/// 歌词列在右半幅；同样用强调色的播放键 / 进度条都在视口之外，按 y 裁掉。
fn accent_centroid_y(buf: &SharedPixelBuffer<Rgba8Pixel>) -> Option<f32> {
    let (w, h) = (buf.width() as usize, buf.height() as usize);
    let mut sum_y = 0f64;
    let mut n = 0u64;
    for (y, row) in buf.as_slice().chunks_exact(w).enumerate() {
        // 视口按比例取：顶部标题栏带之下、底部控制台之上
        let fy = y as f32 / h as f32;
        if !(0.06..0.87).contains(&fy) {
            continue;
        }
        for px in row.iter().skip(w / 2) {
            let (r, g, b) = (i32::from(px.r), i32::from(px.g), i32::from(px.b));
            if r > 150 && r - g > 60 && r - b > 60 {
                sum_y += y as f64;
                n += 1;
            }
        }
    }
    (n > 0).then(|| (sum_y / n as f64) as f32)
}

/// 12 行歌词，奇数行带翻译（行高不等，贴近真实词表）。
fn lyric_rows() -> Vec<LyricRow> {
    (0..12)
        .map(|i| LyricRow {
            timestamp_ms: 8000 + i * 6500,
            text: format!("歌词第{}行 希望有羽毛和翅膀", i + 1).into(),
            translation: if i % 2 == 1 {
                "Wish I had feathers and wings".into()
            } else {
                String::new().into()
            },
        })
        .collect()
}

#[test]
fn lyrics_open_at_the_current_line() {
    // SAFETY: 单线程测试进程内串行执行（本文件仅一个测试）；值唯一。
    unsafe {
        std::env::set_var(
            "HMP_IPC_ENDPOINT",
            "/tmp/hmp-test-lyric-follow-bogus/hmp.sock",
        );
    }
    // 取像素需要软件渲染器：`init_no_event_loop()` 的缺省后端不实现 take_snapshot。
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .expect("platform already initialized");

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let runtime = Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap());
    let prefs = Arc::new(std::sync::Mutex::new(hmp_desktop::prefs::Prefs::default()));
    hmp_desktop::player_bridge::bind(&ui, runtime, prefs);

    // 回归场景：词表与当前行（第 9 行）先就位，播放页后才挂载。
    // 无曲目时播放页是空态（不渲染歌词列），先给一首「在播」。
    let player = Player::get(&ui);
    player.set_has_track(true);
    player.set_title("希望有羽毛和翅膀".into());
    player.set_artists("知更鸟 / HOYO-MiX".into());
    player.set_playing(true);
    player.set_duration_ms(229_000);
    player.set_position_ms(61_000);
    let np = NowPlaying::get(&ui);
    np.set_lyrics(slint::ModelRc::new(slint::VecModel::from(lyric_rows())));
    np.set_lyrics_generation(1);
    np.set_active_line(8);
    settle(200);

    player.invoke_show_overlay();
    settle(480);

    let snapshot = ui.window().take_snapshot().unwrap();
    let centroid = accent_centroid_y(&snapshot).unwrap_or(-1.0);
    assert!(
        (320.0..=415.0).contains(&centroid),
        "进页应直达当前句（强调色质心应在视口中部 320–415），实际 {centroid}"
    );

    // 对照：换行后仍跟随（`changed is-active` 路径）。
    np.set_active_line(3);
    settle(1600);
    let snapshot = ui.window().take_snapshot().unwrap();
    let centroid = accent_centroid_y(&snapshot).unwrap_or(-1.0);
    assert!(
        (320.0..=415.0).contains(&centroid),
        "换行后应跟到新的当前句，实际 {centroid}"
    );
}
