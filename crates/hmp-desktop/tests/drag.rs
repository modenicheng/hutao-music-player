//! 拖动链路回归测试（testing backend + 合成指针，单测试单进程）。
//!
//! 守护三件事：
//! 1. 主进度条拖拽纯本地回显：按下→拖动全程不发 seek，松手恰好发一次
//!    （此前逐移动事件 seek 一次 = IPC 风暴 + daemon 推送回写打架，拖不动）；
//! 2. 抓取被打断（PointerExited → Cancel）时放弃拖拽且不发 seek；
//! 3. Scroll thumb 1:1 拖拽：等长鼠标位移产生等长内容位移，且位移量
//!    与"点轨道跳转"的位移映射一致（此前 thumb TouchArea 在移动参考系内，
//!    稳态半速跟随、越拖越脱节）。
//!
//! 几何（1280×800、layout-gap=8、player-bar-height=96、sidebar 展开宽 224）：
//! - 内容浮板 x=240..1272、y=8..688 → 滚动条命中条 x=1262..1272；
//! - PlayerBar 与内容列同宽（x=240、宽 1032），进度条行在最上
//!   （y=696..702，命中区 y=692..706）→ 百分比 = (x-240)/1032。

use std::sync::{Arc, Mutex};

use slint::{platform::WindowEvent, ComponentHandle, Global, LogicalPosition};

/// 进度条（内容列）原点 x 与宽度
const BAR_X: f32 = 240.0;
const BAR_W: f32 = 1032.0;

fn press_bar(ui: &hmp_desktop::AppWindow, percent: f32) {
    let x = BAR_X + BAR_W * percent;
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: LogicalPosition::new(x, 699.0),
        button: slint::platform::PointerEventButton::Left,
    });
}

fn move_bar(ui: &hmp_desktop::AppWindow, percent: f32) {
    let x = BAR_X + BAR_W * percent;
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, 699.0) });
}

fn release_bar(ui: &hmp_desktop::AppWindow, percent: f32) {
    let x = BAR_X + BAR_W * percent;
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(x, 699.0),
        button: slint::platform::PointerEventButton::Left,
    });
}

fn move_to(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
}

fn press(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: LogicalPosition::new(x, y),
        button: slint::platform::PointerEventButton::Left,
    });
}

fn release(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(x, y),
        button: slint::platform::PointerEventButton::Left,
    });
}

fn settle() {
    for _ in 0..40 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

#[test]
fn drag_progress_and_scroll_thumb() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data(&ui);
    settle();

    // —— 1. 主进度条：拖拽本地回显，松手一次 seek ———
    let player = hmp_desktop::Player::get(&ui);
    player.set_has_track(true);
    player.set_duration_ms(200_000);
    player.set_progress(0.0);

    let seeks: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let seeks = Arc::clone(&seeks);
        player.on_seek_percent(move |percent| {
            seeks.lock().unwrap().push(percent);
        });
    }

    // 按下 0.5 处：进入拖拽、本地回显，不发 seek
    press_bar(&ui, 0.5);
    assert!(player.get_seeking(), "按下后应处于拖拽态");
    let dp = player.get_drag_progress();
    assert!((dp - 0.5).abs() < 0.01, "drag-progress 应为 0.5，实际 {dp}");
    assert!(seeks.lock().unwrap().is_empty(), "拖拽中不得发 seek");

    // 拖动到 0.75：仍纯本地，旧进度推送（daemon 10Hz 回写）不来捣乱
    move_bar(&ui, 0.75);
    assert!(player.get_seeking());
    let dp = player.get_drag_progress();
    assert!((dp - 0.75).abs() < 0.01, "drag-progress 应为 0.75，实际 {dp}");
    assert!(seeks.lock().unwrap().is_empty(), "移动事件不得发 seek");

    // 松手：恰好一次 seek，拖拽态退出
    release_bar(&ui, 0.75);
    assert!(!player.get_seeking());
    let seeked = seeks.lock().unwrap().clone();
    assert_eq!(seeked.len(), 1, "松手应恰好 seek 一次，实际 {seeked:?}");
    assert!((seeked[0] - 0.75).abs() < 0.01);

    // 抓取被打断（指针离开窗口 → Exit → Cancel）：放弃拖拽、不 seek
    press_bar(&ui, 0.2);
    assert!(player.get_seeking());
    ui.window().dispatch_event(WindowEvent::PointerExited);
    assert!(!player.get_seeking(), "取消后应退出拖拽态");
    assert_eq!(seeks.lock().unwrap().len(), 1, "取消不得发 seek");

    // —— 2. Scroll thumb：1:1 拖拽 ———
    // 播种足够多曲目使内容高度 ≫ 视口（400 行 ≥ 8×680px），thumb 必然存在
    {
        let data = hmp_desktop::Data::get(&ui);
        let rows: Vec<hmp_desktop::TrackRow> = (0..400)
            .map(|i| hmp_desktop::TrackRow {
                mid: format!("track-{i}").into(),
                source: 0,
                title: format!("曲目 {i}").into(),
                artists: "测试歌手".into(),
                artist_mid: "".into(),
                album: "".into(),
                album_mid: "".into(),
                duration_ms: 200_000,
                quality: "".into(),
                cover: slint::Image::default(),
            })
            .collect();
        let count = rows.len() as i32;
        data.set_liked(slint::ModelRc::new(slint::VecModel::from(rows)));
        data.set_liked_count(count);
    }
    settle();

    let thumb_x = 1267.0;
    // 滚动深度标尺：点击指针下的表格行 → play-tracks 回带行 index。
    // 行高 39px，index 差 × 39 = 内容位移（窗口坐标测不出深度：命中行
    // 的窗口 y 恒在指针附近，与滚动深度无关）。
    let plays: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let plays = Arc::clone(&plays);
        player.on_play_tracks(move |_tracks, start| {
            plays.lock().unwrap().push(start);
        });
    }
    fn row_index_at_pointer(
        ui: &hmp_desktop::AppWindow,
        plays: &Arc<Mutex<Vec<i32>>>,
    ) -> i32 {
        move_to(ui, 640.0, 300.0);
        press(ui, 640.0, 300.0);
        release(ui, 640.0, 300.0);
        *plays.lock().unwrap().last().expect("行点击应触发 play-tracks")
    }

    // 点轨道跳转：按下即吸附（thumb 中心到指针），两次落点相距 200px
    // → 内容位移 = 200px × 放大率（thumb ≪ 轨道，必然 > 1）
    press(&ui, thumb_x, 208.0);
    release(&ui, thumb_x, 208.0);
    let i1 = row_index_at_pointer(&ui, &plays);
    press(&ui, thumb_x, 408.0);
    release(&ui, thumb_x, 408.0);
    let i2 = row_index_at_pointer(&ui, &plays);
    let jump_rows = i2 - i1;
    assert!(jump_rows > 20, "点轨道跳转应大幅下移（200px×放大率），实际 {jump_rows} 行");

    // 回到 408 落点，再按住拖动 +100px：内容位移应为 100px × 同一放大率
    // ⇔ 行数增量 = jump_rows / 2（1:1 拖拽；半速 bug 时 ≈ 1/4）
    press(&ui, thumb_x, 408.0);
    move_to(&ui, thumb_x, 468.0);
    move_to(&ui, thumb_x, 508.0);
    release(&ui, thumb_x, 508.0);
    let i3 = row_index_at_pointer(&ui, &plays);
    let drag_rows = i3 - i2;
    let ratio = drag_rows as f32 / jump_rows as f32;
    assert!(
        (ratio - 0.5).abs() < 0.05,
        "拖动 +100px 的行数增量应为点跳 200px 的一半（1:1 拖拽）。\
         drag_rows={drag_rows} jump_rows={jump_rows} ratio={ratio}"
    );
}
