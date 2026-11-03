//! S10 退出段看门狗：`hmp quit`/X 关窗后 hmp-desktop.exe 挂死成不可杀僵尸
//! 问题的**纵深防御**件（真根因修复在 `backend::BackendRuntime`，见其模块
//! doc 与 worktree FINDINGS.md §1.2）。
//!
//! 历史根因（2026-10-24 cdb 僵尸栈实锤）：main 返回 → exit() → ExitProcess
//! 先终止其余线程（含 tokio blocking 池）→ `LdrShutdownProcess` 跑主线程
//! TLS 析构 → `bridge::schedule_refresh` 的 thread_local `slint::Timer` 闭包
//! drop `Arc<BackendRuntime>`（最后一个引用）→ `Runtime::drop` →
//! `BlockingPool::shutdown` 在 oneshot 上死等**已被杀死的** blocking 线程 →
//! `WaitOnAddress` 永久挂起（且不可杀：主线程困在 loader 关停期）。
//! BackendRuntime 改持 `&'static` Runtime 后，任何路径都不再执行
//! Runtime::drop——本模块只是给退出段的**用户态**挂起留一道兜底。
//!
//! 语义边界（勿扩大预期）：
//! - 只对**用户态**挂起有效（主线程卡在锁/阻塞 IO、guard 未解除）；
//! - 内核态卡死或 loader 关停期（LdrShutdownProcess/FLS 析构）卡死下，
//!   `std::process::exit` 自身也会卡在进程 rundown——历史上正是这种形态，
//!   任何看门狗都救不了，只能靠根因消灭；
//! - 覆盖边界：main 尾部 disarm 之后的静态析构段（CRT atexit/静态 drop）
//!   不在覆盖内（当前无已知挂起面：tracing 无缓冲 sink、rusqlite 即开即弃）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// 退出段看门狗预算：arm→disarm 的清理路径（drop 链 + 引用计数递减）正常
/// <100ms，2s 为极端调度余量（S10 审核实施要求给定值）。
pub const EXIT_WATCHDOG: Duration = Duration::from_secs(2);

/// 武装退出看门狗：`timeout` 内未解除（[`ExitGuard::disarm`]/drop）则在独立
/// 线程调用 `exit_fn`（生产路径 = `std::process::exit(0)`；测试注入 channel）。
///
/// 线程 detach：正常路径进程先退，线程随进程消亡，不 join（join 反而把清理
/// 串行化、平添一个可挂起点）。
pub fn arm(timeout: Duration, exit_fn: impl FnOnce() + Send + 'static) -> ExitGuard {
    let done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&done);
    // 退出路径上 spawn 失败不该 panic（那会以另一种方式破坏退出）：降级为
    // 无看门狗退出——只失去用户态兜底，根因修复（T7）不受影响。
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
