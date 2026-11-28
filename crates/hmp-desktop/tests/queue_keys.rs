//! 播放列表抽屉键盘导航 + 清空两步确认（testing backend，单测试单进程）。
//!
//! 守护 ui/queue-drawer.slint：
//! 1. 行列表是 roving 光标：获得焦点即落在首个可见行，↑/↓ 移动并滚动跟随
//!    （按绝对下标记，行窗随滚动换窗），Enter 播放该位置，Delete 从队列移除；
//! 2. 「清空」两步确认：首次点击/Enter 只进入确认态，3s 内再点才真清；超时
//!    撤回、关闭抽屉撤回——上千首队列误触一次就没了且无从恢复。
//!
//! 几何（1280×800，抽屉主界面态）：面板 x=892..1272、y=40..688；列表视口高
//! 648 − 69（内距 12 + 头部 56 + 发丝线 1）= 579，行距 49。头部右侧「关闭」
//! 钮 x≈1227..1259，「清空」钮在其左 x≈1167..1219，y≈52..108。
//!
//! 到达列表：根作用域在元素树最前，Shift+Tab 从树尾倒着走「关闭窗口 →
//! 最大化 → 最小化 → 抽屉关闭 → 清空 → 行列表」共 6 次。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, SharedString, platform::WindowEvent};

use hmp_desktop::{Player, QueueRow, QueueWindow};

fn settle(ms: u32) {
    for _ in 0..(ms / 16).max(1) {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn tap(ui: &hmp_desktop::AppWindow, k: impl Into<SharedString>) {
    let k: SharedString = k.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: k.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: k });
}

fn shift_tab(ui: &hmp_desktop::AppWindow) {
    tap_hold(ui, slint::platform::Key::Shift, slint::platform::Key::Tab);
}

fn tap_hold(ui: &hmp_desktop::AppWindow, modifier: slint::platform::Key, k: slint::platform::Key) {
    let modifier: SharedString = modifier.into();
    let text: SharedString = k.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: modifier.clone(),
    });
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: modifier });
}

fn click(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    for pressed in [true, false] {
        ui.window().dispatch_event(if pressed {
            WindowEvent::PointerPressed {
                position,
                button: slint::platform::PointerEventButton::Left,
            }
        } else {
            WindowEvent::PointerReleased {
                position,
                button: slint::platform::PointerEventButton::Left,
            }
        });
    }
}

const CLEAR_BTN: (f32, f32) = (1193.0, 80.0);

#[test]
fn queue_drawer_keys_and_clear_confirm() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data_offline(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );
    settle(320);

    let player = Player::get(&ui);
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
    player.set_queue(slint::ModelRc::new(slint::VecModel::from(queue)));

    let plays: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));
    let removes: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));
    let clears: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    {
        let plays = Arc::clone(&plays);
        player.on_play_at(move |index| plays.lock().unwrap().push(index));
    }
    {
        let removes = Arc::clone(&removes);
        player.on_remove_at(move |index| removes.lock().unwrap().push(index));
    }
    {
        let clears = Arc::clone(&clears);
        player.on_clear_queue(move || *clears.lock().unwrap() += 1);
    }
    // hide-queue / toggle-queue：生产由播放桥写回状态（player_bridge.rs），测试自接
    {
        let ui_weak = ui.as_weak();
        player.on_hide_queue(move || {
            if let Some(ui) = ui_weak.upgrade() {
                Player::get(&ui).set_queue_visible(false);
            }
        });
        let ui_weak = ui.as_weak();
        player.on_toggle_queue(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let player = Player::get(&ui);
                player.set_queue_visible(!player.get_queue_visible());
            }
        });
    }

    // —— 1. roving 光标 ——
    player.set_queue_visible(true);
    settle(300);
    for _ in 0..6 {
        shift_tab(&ui);
    }
    // 获得焦点即落在首个可见行；Enter 播放该位置
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *plays.lock().unwrap(),
        vec![0],
        "获得焦点后 Enter 应播放首个可见行"
    );
    for _ in 0..3 {
        tap(&ui, slint::platform::Key::DownArrow);
    }
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        plays.lock().unwrap().last(),
        Some(&3),
        "↓×3 后 Enter 播放第 3 行"
    );

    // ↓×60 → 第 63 行在视口外：列表滚动跟随，行窗换窗（window-start ≈ 44）
    for _ in 0..60 {
        tap(&ui, slint::platform::Key::DownArrow);
    }
    settle(300);
    let start = QueueWindow::get(&ui).get_start();
    assert!(
        (40..=48).contains(&start),
        "光标走到深处时行窗应随之换窗，实际 window-start={start}"
    );
    // Delete = 从队列移除光标行（第 63 行；后一行顶上来）
    tap(&ui, slint::platform::Key::Delete);
    assert_eq!(
        removes.lock().unwrap().last(),
        Some(&63),
        "Delete 应移除光标行"
    );
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        plays.lock().unwrap().last(),
        Some(&63),
        "移除后光标停在原位置（现为原第 64 行）"
    );

    // —— 2. 清空两步确认（键盘）———
    // 行列表的下一站（Tab：元素树顺序 = 行列表 → 清空 → 关闭）是「清空」
    tap(&ui, slint::platform::Key::Tab);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *clears.lock().unwrap(),
        0,
        "首次 Enter 只进入确认态，不得清空"
    );
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(*clears.lock().unwrap(), 1, "确认态下再按 Enter 才清空");

    // 超时撤回：确认态 3s 后回到普通态
    tap(&ui, slint::platform::Key::Return);
    settle(3400);
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(
        *clears.lock().unwrap(),
        1,
        "确认态超时后 Enter 应重新进入确认态"
    );
    tap(&ui, slint::platform::Key::Return);
    assert_eq!(*clears.lock().unwrap(), 2);

    // —— 鼠标同样两步 ——
    // ESC 关抽屉（同时撤回确认态）→ 重开 → 点一次不应清空
    tap(&ui, slint::platform::Key::Escape);
    settle(400);
    assert!(!player.get_queue_visible(), "ESC 应关闭抽屉");
    player.set_queue_visible(true);
    settle(300);
    click(&ui, CLEAR_BTN.0, CLEAR_BTN.1);
    assert_eq!(*clears.lock().unwrap(), 2, "首次点击只进入确认态");
    click(&ui, CLEAR_BTN.0, CLEAR_BTN.1);
    assert_eq!(*clears.lock().unwrap(), 3, "3s 内再点才清空");

    // 确认态下关闭抽屉 → 重开后是普通态
    click(&ui, CLEAR_BTN.0, CLEAR_BTN.1);
    tap(&ui, slint::platform::Key::Escape);
    settle(400);
    player.set_queue_visible(true);
    settle(300);
    click(&ui, CLEAR_BTN.0, CLEAR_BTN.1);
    assert_eq!(*clears.lock().unwrap(), 3, "关闭抽屉应撤回确认态");
    click(&ui, CLEAR_BTN.0, CLEAR_BTN.1);
    assert_eq!(*clears.lock().unwrap(), 4);

    // 抽屉打开时键盘焦点已交还根作用域：空格是播放/暂停，不是再按一次「清空」
    let toggles: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    {
        let toggles = Arc::clone(&toggles);
        player.on_toggle_play(move || *toggles.lock().unwrap() += 1);
    }
    tap(&ui, " ");
    assert_eq!(
        *toggles.lock().unwrap(),
        1,
        "鼠标用过抽屉后空格应是播放/暂停"
    );
    assert_eq!(*clears.lock().unwrap(), 4, "空格不得再触发清空");
}
