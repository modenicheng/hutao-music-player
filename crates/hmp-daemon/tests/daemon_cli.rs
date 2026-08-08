//! `serve.rs` 编排集成测试（真实 GStreamer 环境；无音频设备的 CI 上跳过）。
//!
//! 协议层（帧编解码 / socket 服务器 / 引擎仲裁）已由 hmp-daemon 内 lib 级单测
//! 覆盖（`server.rs` / `engine.rs`），此处只验证 Task 5 交付的 `serve.rs`
//! 编排闭环：起 daemon → socket 就绪 → Status 应答 → SIGTERM 优雅退出 → socket 清理。
//!
//! `run_background` 返回 `Box<dyn Error>`（非 `Send`），不能 `spawn` 到
//! 多线程 runtime，故在主线程 `block_on` 运行 daemon，另起交互线程做
//! socket 往返与信号触发。

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

/// 等待 socket 可连接（同步轮询）。
fn wait_for_socket(path: &PathBuf, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if path.exists() && std::os::unix::net::UnixStream::connect(path).is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "需要真实 GStreamer/音频环境（真机验收项）"]
fn serve_boots_answers_status_and_cleans_up_on_sigterm() {
    // 隔离 socket：XDG_RUNTIME_DIR 指向临时目录（daemon 的 socket_path 读该变量）。
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: 测试独占该进程；serve::run_background 在 spawn 前读取此变量。
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", dir.path()) };
    let socket = hmp_daemon::server::socket_path();

    // 交互线程：等 socket 就绪 → Status 往返 → 触发 SIGTERM。
    let interact_socket = socket.clone();
    let interact = std::thread::spawn(move || {
        assert!(
            wait_for_socket(&interact_socket, Duration::from_secs(10)),
            "daemon 未在 10s 内就绪"
        );
        let mut stream = std::os::unix::net::UnixStream::connect(&interact_socket).unwrap();
        let frame = hmp_core::ipc::encode_frame(&hmp_core::Request::Status).unwrap();
        stream.write_all(&frame).unwrap();
        let mut buf = vec![0u8; 1 << 20];
        let n = stream.read(&mut buf).unwrap();
        let resp: hmp_core::Response = hmp_core::ipc::decode_frame(&buf[..n]).unwrap();
        assert!(
            matches!(resp, hmp_core::Response::Status(_)),
            "Status 应答异常: {resp:?}"
        );
        // 等信号处理器装好（Status 应答意味着 serve 已 accept，信号任务在其前 spawn）。
        std::thread::sleep(Duration::from_millis(100));
        unsafe { libc::raise(libc::SIGTERM) };
    });

    // 主线程：跑 daemon 到优雅退出。
    let rt = tokio::runtime::Runtime::new().unwrap();
    let res = rt.block_on(async { hmp_daemon::serve::run_background().await });
    res.expect("run_background 返回错误");
    interact.join().expect("交互线程 panicked");
    assert!(
        !socket.exists(),
        "退出后 socket 未清理: {}",
        socket.display()
    );
}
