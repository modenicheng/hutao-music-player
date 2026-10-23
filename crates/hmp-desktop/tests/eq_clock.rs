//! EqClock 播放门控回归（S9 R3 / T3a+T3b）。
//!
//! primitives.slint 的 EqClockHost Timer 由 `running: Player.playing` 门控：
//! 暂停/停止期停表（静止期 33ms 唤醒归零），播放期常跑。本文件验证门控
//! 往返（冻结/复活/无粘死）；播放期正例与 hover_slider.rs:189-195 的既有
//! 探针互补。
//!
//! 注意：testing backend 的 platform 是进程级单例，本文件只放一个 #[test]。
//! mock 时间下门控翻转有一拍延迟：`running` 绑定经 changed 回调接线，而
//! `mock_elapsed_time` 内 run_change_handlers 在 Timer 激活之后执行——
//! 翻转 playing 后需先推一拍「接线」再推进「点火」（真实事件循环迭代
//! 顺序下无此可见性）。

use slint::Global;

/// 推进 mock 时间一拍（让 changed 回调接线 Timer.running 绑定）。
fn tick(ms: u64) {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
}

#[test]
fn eq_clock_gated_by_player_playing() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = hmp_desktop::AppWindow::new().unwrap();
    let player = hmp_desktop::Player::get(&ui);
    let eq = hmp_desktop::EqClock::get(&ui);

    // 1. 初始（playing=false）：时钟冻结（t 为毫秒 i64）
    assert!(!player.get_playing(), "AppWindow 初始应未播放");
    assert_eq!(eq.get_t(), 0, "初始 t 应为 0ms");
    tick(200);
    assert_eq!(eq.get_t(), 0, "暂停期 Timer 停表，mock 推进不应自增 t");

    // 2. 起播：时钟复活前进（首拍接线 + 次拍点火）
    player.set_playing(true);
    tick(16);
    let t0 = eq.get_t();
    tick(100);
    let t1 = eq.get_t();
    assert!(t1 > t0, "播放期 EqClock 应前进：{t0}ms → {t1}ms");
    assert!(t1 > 0, "t 应已离开 0：{t1}ms");

    // 3. 暂停→恢复往返：t 单调不回退、恢复继续前进、再暂停即冻结
    player.set_playing(false);
    let t_at_pause = eq.get_t();
    tick(16);
    tick(150);
    assert_eq!(eq.get_t(), t_at_pause, "再次暂停即冻结（门控无粘死）");
    player.set_playing(true);
    tick(16);
    tick(100);
    let t2 = eq.get_t();
    assert!(t2 > t1, "恢复后应从当前值继续前进：{t1}ms → {t2}ms");
    assert!(
        t2 <= t_at_pause + 100,
        "恢复后前进量应 ≈ 推进的 mock 时间（不跳跃补时）：{t_at_pause}ms → {t2}ms"
    );
}
