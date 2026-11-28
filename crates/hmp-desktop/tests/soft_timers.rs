//! 软卸载 / 播放页定时器回归测试（testing backend + 合成事件，单测试单进程）。
//!
//! 守护 PORTING「Timer restart 陷阱」：`.slint` 里的 `timer.restart()` 编译为
//! `slint::Timer::restart()`，对**从未启动过**的 `Timer { running: false }`
//! 是空操作（Slint 1.18 实测；1.17→1.18 依赖轮后下列行为曾全部静默失效）：
//!
//! 1. 队列抽屉软卸载：关闭后淡出播完必须真的卸载——面板常驻挂载（opacity 0
//!    仍参与命中）会拦走内容区右缘约 380px 的点击与拖拽；
//! 2. 播放页软卸载：关闭后必须卸载——整棵树常驻则歌词跟随 16ms 定时器被
//!    10Hz 状态推送反复拉起，后台空转；
//! 3. 评论跳转键：歌词首屏 ⇄ 评论区双向平滑滚动；
//! 4. 歌词手动滚动后 3s 恢复自动跟随。
//!
//! 几何（1280×800，播放页 overlay 打开）：内容列高 = 800 − 控制台 104 =
//! 696；舞台阅读列 1088 居中（x=96..1184），左列宽 min(36% 舞台高, 40%,
//! 336) ≈ 250.6，歌词列自 x ≈ 136+250.6+32 ≈ 419 起；歌词首尾留白
//! 45%/52%，首行激活时其中心在视口竖直中心 ≈ 348。

use std::sync::Arc;

use slint::{
    ComponentHandle, Global, Model, ModelRc, SharedString, VecModel, platform::WindowEvent,
};

use hmp_desktop::{NowPlaying, Player, QueueRow};

fn settle(ms: u32) {
    for _ in 0..(ms / 16).max(1) {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn wheel(ui: &hmp_desktop::AppWindow, x: f32, y: f32, dy: f32, notches: u32) {
    for _ in 0..notches {
        ui.window().dispatch_event(WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(x, y),
            delta_x: 0.0,
            delta_y: dy,
        });
    }
}

#[test]
fn soft_unmount_and_overlay_timers() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data_offline(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );
    settle(320);

    let player = Player::get(&ui);
    let np = NowPlaying::get(&ui);

    // —— 1. 队列抽屉：关着不挂载 → 打开挂载 → 关闭后淡出保持期仍挂载 → 卸载 ———
    assert!(!ui.get_queue_mounted(), "初始关闭态不应挂载面板");
    let queue: Vec<QueueRow> = (0..120)
        .map(|i| QueueRow {
            mid: format!("q-{i}").into(),
            source: 0,
            title: format!("队列曲目 {i}").into(),
            artists: "歌手".into(),
            artist_mid: "".into(),
            album: "".into(),
            album_mid: "".into(),
            duration_ms: 200_000,
            quality: "".into(),
            cover: slint::Image::default(),
        })
        .collect();
    let count = queue.len() as i32;
    player.set_queue(ModelRc::new(VecModel::from(queue)));
    player.set_has_track(true);

    for round in 0..2 {
        player.set_queue_visible(true);
        settle(600);
        assert!(ui.get_queue_mounted(), "第 {round} 轮：打开后面板应挂载");

        player.set_queue_visible(false);
        // changed 回调延迟一拍：推进两拍让保持标志落下，再断言淡出期间仍挂载
        settle(32);
        assert!(ui.get_queue_mounted(), "第 {round} 轮：淡出保持期不得卸载");
        settle(600);
        assert!(
            !ui.get_queue_mounted(),
            "第 {round} 轮：淡出播完后面板必须卸载（常驻会拦走内容区右缘点击）"
        );
    }
    assert_eq!(
        player.get_queue().row_count(),
        count as usize,
        "队列本体不受影响"
    );

    // —— 2. 播放页：关闭后卸载 ———
    assert!(!ui.get_overlay_mounted(), "初始关闭态不应挂载播放页");
    player.set_overlay_visible(true);
    settle(600);
    assert!(ui.get_overlay_mounted());

    // —— 3. 评论跳转键（播放页打开期间）：歌词首屏 → 评论区 → 回歌词 ———
    let y0 = np.get_overlay_scroll_y();
    assert!(y0.abs() < 1.0, "初始应在歌词首屏，实际 {y0}");
    np.invoke_jump_comments();
    settle(600);
    let y1 = np.get_overlay_scroll_y();
    assert!(
        y1 < -400.0 && y1 > -700.0,
        "跳评论应滚到评论区顶（约一屏 696，受内容高钳制），实际 {y1}"
    );
    np.invoke_jump_comments();
    settle(600);
    let y2 = np.get_overlay_scroll_y();
    assert!(y2.abs() < 1.0, "已在评论区时再按应回歌词首屏，实际 {y2}");

    // —— 4. 歌词手动滚动 → 暂停跟随 → 3s 后恢复 ———
    let lyrics: Vec<hmp_desktop::LyricRow> = (0..24)
        .map(|i| hmp_desktop::LyricRow {
            timestamp_ms: i * 4000,
            text: format!("第 {i} 行歌词").into(),
            translation: SharedString::new(),
        })
        .collect();
    np.set_lyrics(ModelRc::new(VecModel::from(lyrics)));
    np.set_active_line(0);
    settle(600);
    assert!(np.get_lyrics_following(), "换词表后应处于自动跟随");

    wheel(&ui, 780.0, 348.0, -120.0, 3);
    settle(64);
    assert!(!np.get_lyrics_following(), "手动滚动歌词后应暂停自动跟随");
    settle(3 * 1000 + 200);
    assert!(np.get_lyrics_following(), "手动滚动 3s 后应恢复自动跟随");

    // —— 2（续）. 关闭播放页：淡出保持期仍挂载 → 卸载 ———
    player.set_overlay_visible(false);
    settle(32);
    assert!(ui.get_overlay_mounted(), "退场保持期不得卸载");
    settle(600);
    assert!(
        !ui.get_overlay_mounted(),
        "退场播完后播放页必须卸载（常驻则歌词跟随定时器被状态推送反复拉起）"
    );
}
