//! 音量弹层集成测试（testing backend + 合成指针/按键，单测试单进程）。
//!
//! 守护 ui/volume-control.slint 的四件事：
//! 1. **未播放时悬停后能收**：收起宽限曾借用 EqClock 当时钟，而 EqClock
//!    自 S9 R3 起只在播放期走表——暂停/未播放时悬停音量图标后移开，弹层
//!    永远不收。现改用真正的 Timer；
//! 2. **点击钉住 / 点外与 ESC 解除**（Vue pinned 语义）：图标点击 = 静音
//!    切换并钉住弹层；指针在控件外按下或 ESC 解除；
//! 3. **方向键微调**：触发钮是 Tab 停靠点，↑ +5%、↓ −5%；
//! 4. **聚焦即展开**：键盘聚焦触发钮期间弹层保持可见。
//!
//! 几何（1280×800）：PlayerBar x=240..1272、y=696..792；状态卡 y=710..792；
//! 右簇音量钮 36×36 → (1220..1256, 733..769)，中心 (1238, 751)。弹层宽 44
//! 高 160，悬在钮上方 10px → (1216..1260, 563..723)；滑轨命中列 y≈588..712
//! （拖拽 = 1 − 指针在轨内的相对 y）。
//!
//! 指针活动钩子：生产路径是 winit 按下钩子（main.rs），先于事件派发登记；
//! 测试里按同一顺序显式调用 `keynav::pointer_activity`。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, SharedString, platform::WindowEvent};

use hmp_desktop::{Player, keynav};

fn settle(ms: u32) {
    let ticks = (ms / 16).max(1);
    for _ in 0..ticks {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn move_to(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(x, y),
    });
}

fn press(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: slint::LogicalPosition::new(x, y),
        button: slint::platform::PointerEventButton::Left,
    });
}

fn release(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: slint::LogicalPosition::new(x, y),
        button: slint::platform::PointerEventButton::Left,
    });
}

/// 指针先移到目标再按下抬起（真实输入序列必有 Moved 在 Pressed 之前）
fn click(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    move_to(ui, x, y);
    press(ui, x, y);
    release(ui, x, y);
}

fn tap(ui: &hmp_desktop::AppWindow, key: impl Into<SharedString>) {
    let text: SharedString = key.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text });
}

fn shift_tab(ui: &hmp_desktop::AppWindow) {
    let shift = <SharedString as From<_>>::from(slint::platform::Key::Shift);
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: shift.clone(),
    });
    tap(ui, slint::platform::Key::Tab);
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: shift });
}

const TRIGGER: (f32, f32) = (1238.0, 751.0);
/// 滑轨内一点：相对 y ≈ (610−588)/124 → 音量 ≈ 0.82
const RAIL: (f32, f32) = (1238.0, 610.0);

#[test]
fn volume_popover_grace_close_pin_and_keys() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );
    settle(320);

    let player = Player::get(&ui);
    player.set_volume(0.5);
    assert!(!player.get_playing(), "本测试全程未播放（EqClock 停表）");

    // set-volume 记录 + 本地回显（生产路径由 player_bridge 做）
    let sets: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sets = Arc::clone(&sets);
        let ui_weak = ui.as_weak();
        player.on_set_volume(move |value| {
            sets.lock().unwrap().push(value);
            if let Some(ui) = ui_weak.upgrade() {
                Player::get(&ui).set_volume(value);
            }
        });
    }
    let toggles = Arc::new(Mutex::new(0usize));
    {
        let toggles = Arc::clone(&toggles);
        player.on_toggle_play(move || {
            *toggles.lock().unwrap() += 1;
        });
    }
    let snap = |sets: &Arc<Mutex<Vec<f32>>>| sets.lock().unwrap().clone();

    // —— 1. 未播放时悬停展开、移开 180ms 宽限后收起 ———
    move_to(&ui, TRIGGER.0, TRIGGER.1);
    settle(16);
    click(&ui, RAIL.0, RAIL.1);
    {
        let s = snap(&sets);
        assert_eq!(s.len(), 1, "悬停时滑轨应可点，实际 {s:?}");
        assert!(
            (s[0] - 0.82).abs() < 0.03,
            "滑轨点击音量应 ≈0.82，实际 {}",
            s[0]
        );
    }
    move_to(&ui, 640.0, 400.0);
    settle(400);
    click(&ui, RAIL.0, RAIL.1);
    assert_eq!(
        snap(&sets).len(),
        1,
        "移开超过宽限后弹层应已收起：滑轨位置不得再响应"
    );

    // —— 2. 点击图标 = 静音 + 钉住；ESC 解除 ——
    // 生产顺序：winit 按下钩子先登记指针活动，事件才派发到控件
    move_to(&ui, TRIGGER.0, TRIGGER.1);
    settle(16);
    keynav::pointer_activity(&ui);
    click(&ui, TRIGGER.0, TRIGGER.1);
    settle(48);
    assert_eq!(player.get_volume(), 0.0, "点击图标应静音");
    assert_eq!(player.get_volume_pin_count(), 1, "点击后弹层应钉住");

    let unpin_before = player.get_volume_unpin_seq();
    tap(&ui, slint::platform::Key::Escape);
    assert_eq!(
        player.get_volume_unpin_seq(),
        unpin_before + 1,
        "ESC 应广播解除钉住"
    );
    settle(48);
    assert_eq!(player.get_volume_pin_count(), 0, "ESC 后弹层应解除钉住");

    // 再点一次 = 取消静音 + 钉住；控件外按下 → 解除（ESC 不再有弹层可收）
    keynav::pointer_activity(&ui);
    click(&ui, TRIGGER.0, TRIGGER.1);
    settle(48);
    assert!(
        player.get_volume() > 0.01,
        "再次点击应取消静音（恢复静音前音量）"
    );
    assert_eq!(player.get_volume_pin_count(), 1);
    move_to(&ui, 640.0, 400.0);
    keynav::pointer_activity(&ui); // 控件外的按下
    settle(48);
    assert_eq!(player.get_volume_pin_count(), 0, "控件外按下应解除钉住");
    let unpin_before = player.get_volume_unpin_seq();
    tap(&ui, slint::platform::Key::Escape);
    assert_eq!(
        player.get_volume_unpin_seq(),
        unpin_before,
        "无钉住弹层时 ESC 不归它"
    );

    // —— 3/4. 键盘：Shift+Tab 倒着走到音量钮 → 聚焦即展开，↑ +5% ——
    // 根作用域在元素树最前，Shift+Tab 从树尾倒着走：关闭 → 最大化 → 最小化
    // （标题栏三钮）→ 播放条最后一个停靠点（队列为空 = 音量钮）
    let before = player.get_volume();
    let count_before = snap(&sets).len();
    for _ in 0..4 {
        shift_tab(&ui);
    }
    tap(&ui, slint::platform::Key::UpArrow);
    {
        let s = snap(&sets);
        assert_eq!(s.len(), count_before + 1, "↑ 应微调音量，实际 {s:?}");
        assert!((s[count_before] - (before + 0.05)).abs() < 1e-3);
    }
    // 聚焦期间弹层保持展开：指针不在控件上，滑轨仍可点
    settle(16);
    click(&ui, RAIL.0, RAIL.1);
    assert_eq!(
        snap(&sets).len(),
        count_before + 2,
        "键盘聚焦期间弹层应展开（滑轨可点）"
    );

    // 聚焦钮上的空格 = 静音切换（按下该钮），不是播放/暂停
    tap(&ui, " ");
    assert_eq!(player.get_volume(), 0.0, "聚焦音量钮时空格应静音");
    assert_eq!(
        *toggles.lock().unwrap(),
        0,
        "聚焦音量钮时空格不得触发播放/暂停"
    );
}
