//! CLI 进程级集成测试（真机验收：需要真实 GStreamer/音频环境，默认 `#[ignore]`）。
//!
//! 协议层已由 hmp-daemon lib 级测试覆盖（`server.rs` / `engine.rs`）；
//! 此处端到端验证 CLI → daemon 完整链路（spec §6）：
//! `hmp serve --background`（隔离 XDG_RUNTIME_DIR）→ `hmp status` 连接并输出状态行
//! → `hmp quit` → daemon 优雅退出并清理 socket（无需 SIGTERM）。

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn hmp_bin() -> &'static str {
    // cargo 为测试进程注入运行期环境变量（编译期 env! 不可用）。
    Box::leak(Box::new(
        std::env::var("CARGO_BIN_EXE_hmp").expect("CARGO_BIN_EXE_hmp 未注入"),
    ))
}

fn wait_for_socket(path: &PathBuf, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if path.exists() && std::os::unix::net::UnixStream::connect(path).is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 轮询断言条件（测试用同步阻塞）。
fn wait_until(mut cond: impl FnMut() -> bool, timeout: Duration, what: &str) {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return;
        }
        assert!(Instant::now() < deadline, "超时: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "需要真实 GStreamer/音频环境（真机验收项）"]
fn daemon_lifecycle_end_to_end() {
    // 独立 socket 目录（不污染真实 XDG_RUNTIME_DIR）。
    let base = std::env::temp_dir().join(format!("hmp-cli-it-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let socket = base.join("hmp.sock");

    // 1) 起后端（后台、丢弃 stdio，模拟 CLI 自动拉起）。
    let mut daemon = Command::new(hmp_bin())
        .args(["serve", "--background"])
        .env("XDG_RUNTIME_DIR", &base)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hmp serve --background 失败");

    let run = |args: &[&str]| {
        Command::new(hmp_bin())
            .args(args)
            .env("XDG_RUNTIME_DIR", &base)
            .output()
            .expect("运行 hmp 失败")
    };

    // 2) socket 就绪 → hmp status 输出状态行。
    assert!(
        wait_for_socket(&socket, Duration::from_secs(10)),
        "后端未在 10s 内就绪"
    );
    let out = run(&["status"]);
    assert!(
        out.status.success(),
        "hmp status 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("状态:"), "hmp status 缺少状态行: {stdout}");

    // 3) hmp quit：引擎退出 → 终止信号 → daemon 优雅退出并清理 socket（spec §6）。
    let out = run(&["quit"]);
    assert!(
        out.status.success(),
        "hmp quit 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // 优雅退出断言：quit 后进程自行退出（不再需要 SIGTERM），socket 文件被清理。
    wait_until(
        || !socket.exists(),
        Duration::from_secs(5),
        "quit 后 socket 清理",
    );
    let status = daemon.wait().expect("等待 daemon 退出失败");
    assert!(status.success(), "daemon 退出码异常: {status:?}");

    let _ = std::fs::remove_dir_all(&base);
}
