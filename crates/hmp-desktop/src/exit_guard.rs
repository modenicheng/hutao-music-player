//! 退出编排（S10）：`hmp quit`/X 关窗后 hmp-desktop.exe 挂死成不可杀僵尸的修复件。
//!
//! 根因（worktree FINDINGS.md §1.2）：退出路径 femtovg GL context/surface 销毁
//! （`wglDeleteContext`/`eglDestroy*` 等驱动调用）在内核态不返回；TerminateProcess/
//! ExitProcess 都要等线程出内核，进程内**无法事后补救**——必须让销毁调用不发生：
//! - [`install_exit_on_close`]：X 关窗不再 hide（hide 会让窗口在事件循环 Exit
//!   处理时被 suspend → GL 销毁在循环内执行），保持 Visible 直接退出；
//! - main 侧 `std::mem::forget(ui)`：跳过 drop 段的 GL 销毁链（T1，在 main.rs）。
//!
//! [`arm`] 看门狗是**次级防线**：只兜退出段用户态挂起（锁死锁/阻塞 IO 卡住
//! disarm 之前的清理）；对内核态卡死（本缺陷形态）任何 exit 调用同样会卡在
//! ExitProcess rundown——见 FINDINGS §1.2「关键推论」。覆盖边界（O4）：main
//! 尾部 disarm 之后的静态析构段（CRT atexit/静态 drop）不在看门狗覆盖内
//! （当前无已知挂起面：tracing 无缓冲文件 sink、rusqlite 连接即开即弃）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use slint::{CloseRequestResponse, ComponentHandle};

use crate::AppWindow;

/// 退出段看门狗预算：arm→disarm 的正常路径本身 <1ms（forget + 引用计数递减），
/// 2s 为极端调度余量（S10 审核实施要求给定值）。
pub const EXIT_WATCHDOG: Duration = Duration::from_secs(2);

/// 武装退出看门狗：`timeout` 内未解除（[`ExitGuard::disarm`]/drop）则在独立
/// 线程调用 `exit_fn`（生产路径 = `std::process::exit(0)`；测试注入 channel）。
///
/// 语义边界（勿扩大预期）：
/// - 只对**用户态**挂起有效（主线程卡在锁/阻塞 IO、guard 未解除）；
/// - 内核态卡死（S10 根因形态）下 `exit_fn` 自身也会卡在进程 rundown——
///   主防线是 main.rs 的 leak-on-exit，本函数只是纵深；
/// - 线程 detach：正常路径进程先退，线程随进程消亡，不 join（join 反而把
///   清理串行化、平添一个可挂起点）。
pub fn arm(timeout: Duration, exit_fn: impl FnOnce() + Send + 'static) -> ExitGuard {
    let done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&done);
    // 退出路径上 spawn 失败不该 panic（那会以另一种方式破坏退出）：降级为
    // 无看门狗退出——只失去用户态兜底，主防线（leak-on-exit）不受影响。
    if thread::Builder::new()
        .name("hmp-exit-watchdog".to_string())
        .spawn(move || {
            thread::sleep(timeout);
            if !flag.load(Ordering::Acquire) {
                exit_fn();
            }
        })
        .is_err()
    {
        tracing::warn!("exit watchdog spawn failed; exiting without watchdog");
    }
    ExitGuard { done }
}

/// 看门狗解除句柄：drop（含显式 [`ExitGuard::disarm`]）即置位完成标志，
/// 看门狗线程醒来后不再触发 exit_fn。
pub struct ExitGuard {
    done: Arc<AtomicBool>,
}

impl ExitGuard {
    /// 显式解除（清理完成）。语义与 drop 相同——保留具名调用是为了在 main
    /// 退出编排里把「清理完成」写明（而非隐式离开作用域）。
    pub fn disarm(self) {}
}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
    }
}

/// X 关窗统一到「Visible 退出」（S10 T3）。
///
/// slint 默认行为：CloseRequested → hide → 最后窗口关闭退出 → 事件循环 Exit
/// 处理时 hidden 窗口被 `suspend_all_hidden_windows` suspend（GL context 销毁
/// **在事件循环内**执行，与 quit 路径 drop 段同族风险；i-slint-backend-winit
/// 1.17.1 event_loop.rs L118-132/L556）。装此回调后：CloseRequested → 直接
/// `quit_event_loop()`，窗口保持 Visible → Exit 处理跳过 suspend → run() 返回
/// （其尾部 hide 在 Windows 只走 winit SW_HIDE、不含 GL，FINDINGS §1.2-3）
/// → main 的 forget(ui)（T1）收口——**两条退出路径均无 GL 销毁调用**。
///
/// `quit_event_loop` 失败仅当事件循环已拆（进程已在退出，X 无从点击）：静默
/// 保持窗口，无用户可感窗口期。UX 不变：X 仍然退出整个应用（此前默认
/// hide→quit 也是退出），窗口随进程消亡。
pub fn install_exit_on_close(ui: &AppWindow) {
    ui.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        CloseRequestResponse::KeepWindowShown
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// 未 disarm：预算附近触发 exit_fn（注入 channel 观察，不真 exit）。
    #[test]
    fn fires_when_not_disarmed() {
        let (tx, rx) = mpsc::channel();
        // 绑定具名变量持活——`let _ =` 会立即 drop（Drop 置位）使看门狗静默。
        let _guard = arm(Duration::from_millis(50), move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(1))
            .expect("未 disarm 的看门狗应在预算内触发 exit_fn");
    }

    /// disarm 后：看门狗线程醒来见完成标志，不触发 exit_fn。
    #[test]
    fn silent_after_disarm() {
        let (tx, rx) = mpsc::channel();
        let guard = arm(Duration::from_millis(50), move || {
            let _ = tx.send(());
        });
        guard.disarm();
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "disarm 后看门狗不应触发"
        );
    }
}
