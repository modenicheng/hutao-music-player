//! 播放页喜欢键接线回归（testing backend + 合成点击，单测试单进程——
//! init_no_event_loop 进程级单例，context_menu.rs 同约束）。
//!
//! 守护两件事：
//! 1. 心形点击触发 `NowPlaying.toggle-liked` 回调（Slint → Rust 意图链通；
//!    真实处理在 player_bridge::bind 的 on_toggle_liked，此处测试自绑观察者
//!    只验证 Slint 侧契约）；
//! 2. 点击**不再本地翻转** `NowPlaying.liked`（mock 态已删除——liked 值由
//!    Rust 权威写入：换曲回填库事实 + 点击乐观翻转）。
//!
//! 命中点标定：窗口 1280×800 + 短标题单曲下，40px 命中区实测覆盖
//! x[348..384] y[456..492]（testing backend 精扫探针，步长 4），取几何中心
//! (366, 474)；标题字号随 Theme.win-width 推导，测试种子固定短标题一行高，
//! 布局确定性成立。布局代际收敛依赖 mock 时间步进（PORTING.md 坑位）。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, LogicalPosition, platform::WindowEvent};

/// 心形 40×40 命中区几何中心（推导见文件头注释）。
const LIKE_X: f32 = 366.0;
const LIKE_Y: f32 = 474.0;

fn settle(steps: usize) {
    for _ in 0..steps {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

fn click(ui: &hmp_desktop::AppWindow, x: f32, y: f32) {
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: LogicalPosition::new(x, y),
        button: slint::platform::PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(x, y),
        button: slint::platform::PointerEventButton::Left,
    });
}

#[test]
fn like_key_click_fires_callback_without_local_flip() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));

    // 播种当前曲 + 打开播放页 overlay（舞台 `if Player.has-track` 挂载）
    {
        let player = hmp_desktop::Player::get(&ui);
        player.set_current_mid("like-test-mid".into());
        player.set_title("测试曲目".into());
        player.set_has_track(true);
        player.set_overlay_visible(true);
        hmp_desktop::NowPlaying::get(&ui).set_liked(false);
    }

    // 观察者：测试自绑回调扮演 Rust 处理器（真实绑定在 player_bridge::bind）
    let toggles: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    {
        let toggles = Arc::clone(&toggles);
        hmp_desktop::NowPlaying::get(&ui).on_toggle_liked(move || {
            *toggles.lock().unwrap() += 1;
        });
    }

    // 布局 + 入场滑入动画（slide y：Theme.duration-slow）收敛
    settle(100);

    click(&ui, LIKE_X, LIKE_Y);
    settle(4);
    assert_eq!(
        *toggles.lock().unwrap(),
        1,
        "心形点击应触发 toggle-liked 回调恰好一次"
    );
    assert!(
        !hmp_desktop::NowPlaying::get(&ui).get_liked(),
        "点击不得本地翻转 liked（Rust 权威写入，mock 翻转已删除）"
    );

    // 连点：意图链持续上报、本地态仍不漂移
    click(&ui, LIKE_X, LIKE_Y);
    settle(4);
    assert_eq!(*toggles.lock().unwrap(), 2, "第二次点击应再触发回调");
    assert!(!hmp_desktop::NowPlaying::get(&ui).get_liked());
}
