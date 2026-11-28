//! 减少动效开关集成测试（testing backend + 合成按键，单测试单进程）。
//!
//! 守护 Theme.reduce-motion（设置→常规「减少动效」，DESIGN §5「关闭位移类
//! 动效」）：开启后位移类动效直接到位。以播放页的评论跳转为探针——
//! 完整动效下它是 320ms 的插值滚动（跳转后 32ms 仍在途中），减少动效下
//! 同一时刻已落在评论区顶。
//!
//! 顺带钉住开关是可逆的：关掉后位移动效恢复。

use std::sync::Arc;

use slint::{ComponentHandle, Global};

use hmp_desktop::{NowPlaying, Player, Theme};

fn settle(ms: u32) {
    for _ in 0..(ms / 16).max(1) {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }
}

#[test]
fn reduce_motion_makes_displacement_immediate() {
    i_slint_backend_testing::init_no_event_loop();

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    hmp_desktop::bridge::load_data_offline(
        &ui,
        &Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap()),
    );
    settle(320);

    let theme = Theme::get(&ui);
    let player = Player::get(&ui);
    let np = NowPlaying::get(&ui);
    assert!(!theme.get_reduce_motion(), "缺省是完整动效");

    player.set_has_track(true);
    player.set_overlay_visible(true);
    settle(600);
    assert!(ui.get_overlay_mounted());

    // —— 完整动效：跳转后 32ms 仍在途中（未到评论区顶 ≈ −522）———
    np.invoke_jump_comments();
    settle(32);
    let in_flight = np.get_overlay_scroll_y();
    assert!(
        in_flight > -450.0 && in_flight < -1.0,
        "完整动效下评论跳转应是插值滚动（32ms 时在途中），实际 {in_flight}"
    );
    settle(600);
    let arrived = np.get_overlay_scroll_y();
    assert!(arrived < -450.0, "插值最终应到位，实际 {arrived}");

    // —— 减少动效：同一时刻已经直达 ———
    theme.set_reduce_motion(true);
    np.invoke_jump_comments(); // 回歌词首屏
    settle(32);
    let back = np.get_overlay_scroll_y();
    assert!(back.abs() < 1.0, "减少动效下回歌词应直达，实际 {back}");
    np.invoke_jump_comments();
    settle(32);
    let direct = np.get_overlay_scroll_y();
    assert!(
        (direct - arrived).abs() < 1.0,
        "减少动效下评论跳转应直达，实际 {direct}（完整动效终值 {arrived}）"
    );

    // —— 可逆：关掉后位移动效恢复 ———
    theme.set_reduce_motion(false);
    np.invoke_jump_comments();
    np.invoke_jump_comments();
    settle(32);
    let flying = np.get_overlay_scroll_y();
    assert!(
        flying > -450.0 && flying < -1.0,
        "关掉开关后插值滚动应恢复，实际 {flying}"
    );
}
