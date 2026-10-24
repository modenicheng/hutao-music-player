//! S10 退出路径行为回归（testing backend，软件渲染——GL 销毁链在本后端
//! 结构性不存在，死锁本身不可单测；这里钉的是修复的**行为面**）：
//! X 关窗（CloseRequested）→ 直接退出事件循环且**窗口不被 hide**
//! （KeepWindowShown 语义：默认 hide 会让窗口在 Exit 处理时被
//! suspend_all_hidden_windows 拿去跑 GL 销毁——正是 S10 根因的 X 关窗形态，
//! 见 src/exit_guard.rs 模块 doc）。
//!
//! 线程模型：测试线程跑 slint 事件循环；driver 线程经 invoke_from_event_loop
//! 派发 CloseRequested（`ui.window()` 借用非 Send 的 Window，不能跨线程直持，
//! live_daemon.rs 同款约束）。testing backend 单例 → 本文件仅一个 #[test]。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::{ComponentHandle, Weak};

use hmp_desktop::{AppWindow, exit_guard};

type Failures = Arc<Mutex<Vec<String>>>;

#[test]
fn close_requested_quits_without_hiding() {
    i_slint_backend_testing::init_integration_test_with_system_time();

    let ui = AppWindow::new().unwrap();
    // M5：is_visible() 的实现是 strong_component_ref.is_some()（corelib
    // window.rs）——show() 存入自持强引用后 is_visible 才为 true；从未 show
    // 的窗口恒 false，断言会假绿。
    ui.show().unwrap();
    assert!(ui.window().is_visible(), "前置：show 后窗口应可见");

    // 生产同款安装器（T3 复用点：钉的是真行为而非复制粘贴的旧代码）。
    exit_guard::install_exit_on_close(&ui);

    let ui_weak: Weak<AppWindow> = ui.as_weak();
    let failures: Failures = Arc::new(Mutex::new(Vec::new()));

    // driver：等事件循环起来后在 UI 线程派发 CloseRequested；若退出路径失效
    // （回归），10s 兜底 quit 让测试失败而非挂死。
    let driver_failures = Arc::clone(&failures);
    std::thread::spawn(move || {
        let failures = driver_failures;
        std::thread::sleep(Duration::from_millis(200));
        let dispatched = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            }
        })
        .is_ok();
        if !dispatched {
            failures
                .lock()
                .unwrap()
                .push("CloseRequested 派发失败（invoke_from_event_loop）".into());
        }
        std::thread::sleep(Duration::from_secs(10));
        failures
            .lock()
            .unwrap()
            .push("事件循环未在 10s 内退出（X 关窗直退路径失效？）".into());
        let _ = slint::quit_event_loop();
    });

    // 事件循环应因 quit_event_loop 正常返回（handler 内调用，非 hide 链）。
    slint::run_event_loop().expect("事件循环应正常返回");

    let visible = ui.window().is_visible();
    let messages = failures.lock().unwrap();
    assert!(messages.is_empty(), "driver 侧失败: {messages:?}");
    assert!(
        visible,
        "X 关窗后窗口不应被 hide（KeepWindowShown 语义，走了默认 hide 即回归）"
    );
}
