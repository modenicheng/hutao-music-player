//! 回归：daemon 未运行时 `hmp quit` 不得拉起新 daemon（协议审计 F3 Bug 2）。
//!
//! 修复前：quit 走 `connect_or_spawn`，端点 NotFound → spawn `hmp serve
//! --background` → 再 quit；spawn 后 3s 就绪窗口内连不上（慢盘/杀软扫描）
//! 时报 "daemon startup timed out" 且留下刚拉起的 daemon 继续运行。
//! 修复后：quit 走纯连接，端点无人监听 → 打印 "daemon not running" 并以
//! 退出码 0 结束（幂等：目标态已达成），端点保持无人监听。
//!
//! 端点隔离用 `HMP_IPC_ENDPOINT`（serve 端与 CLI 客户端同一 `socket_path()`
//! 实现）：指向必然不存在的唯一路径，不触碰真实 daemon 的默认端点；
//! Windows 上任意路径被 `transport::pipe_name` 确定性映射为
//! `\\.\pipe\hmp-<sanitized>`，Unix 下是 socket 文件路径。

use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn hmp_bin() -> &'static str {
    // cargo 为测试进程注入运行期环境变量（编译期 env! 不可用）。
    Box::leak(Box::new(
        std::env::var("CARGO_BIN_EXE_hmp").expect("CARGO_BIN_EXE_hmp 未注入"),
    ))
}

/// 必然不存在的唯一端点（进程 id + 纳秒时间戳，不与真实 daemon/并行测试冲突）。
fn dead_endpoint() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "hmp-quit-no-spawn-{}-{nanos}.sock",
        std::process::id()
    ))
}

#[test]
fn quit_on_dead_endpoint_succeeds_without_spawning() {
    let endpoint = dead_endpoint();

    let start = Instant::now();
    let out = Command::new(hmp_bin())
        .arg("quit")
        .env("HMP_IPC_ENDPOINT", &endpoint)
        .output()
        .expect("运行 hmp quit 失败");
    let elapsed = start.elapsed();

    // 退出码 0（daemon 未运行 = 目标态已达成），且打印 not-running 提示。
    // 该提示只出自纯连接的 NotFound 分支：若 quit 曾走 spawn 路径，要么
    // 连上刚拉起的 daemon（无此输出），要么 3s 超时后以退出码 1 失败。
    assert!(
        out.status.success(),
        "quit 应成功退出: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("daemon not running"),
        "应打印 daemon not running: {stdout}"
    );
    // 修复后纯连接立即返回；spawn 路径必经历 ≥3s 就绪等待。
    assert!(
        elapsed < Duration::from_secs(3),
        "quit 不应经历 daemon 拉起等待窗口: {elapsed:?}"
    );

    // 幂等：端点仍无人监听，二次 quit 同样成功（若有 daemon 被意外拉起，
    // 它会绑定同一 HMP_IPC_ENDPOINT，此探测会连上并把它退出）。
    let probe = Command::new(hmp_bin())
        .arg("quit")
        .env("HMP_IPC_ENDPOINT", &endpoint)
        .output()
        .expect("二次 quit 失败");
    assert!(
        probe.status.success(),
        "二次 quit 应幂等成功: stdout={} stderr={}",
        String::from_utf8_lossy(&probe.stdout),
        String::from_utf8_lossy(&probe.stderr)
    );
}
