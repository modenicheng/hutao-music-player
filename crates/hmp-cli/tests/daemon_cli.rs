#![cfg(unix)]

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

/// 里程碑 F：本地歌单播放全链路（真实 daemon + 真实音频）。
/// 建歌单 → 加本地 wav → `hmp play playlist:local:<id>` → status 显示播放中。
/// 数据目录隔离：XDG_DATA_HOME 指向临时目录（避免污染真实库）。
#[test]
#[ignore = "需要真实 GStreamer/音频环境（真机验收项）"]
fn library_playlist_plays_locally() {
    let base = std::env::temp_dir().join(format!("hmp-cli-pl-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let socket = base.join("hmp.sock");
    // 隔离运行时/数据/配置目录（测试写真实库会污染用户数据）。
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let config = base.join("config");
    std::fs::create_dir_all(&config).unwrap();

    let mut daemon = Command::new(hmp_bin())
        .args(["serve", "--background"])
        .env("XDG_RUNTIME_DIR", &base)
        .env("XDG_DATA_HOME", &data)
        .env("XDG_CONFIG_HOME", &config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hmp serve --background 失败");
    assert!(
        wait_for_socket(&socket, Duration::from_secs(15)),
        "daemon socket 未就绪"
    );

    // 本地 wav（GStreamer 可播，无需凭证）。
    let wav = base.join("tone.wav");
    write_wav(&wav);

    let run = |args: &[&str]| {
        let out = Command::new(hmp_bin())
            .args(args)
            .env("XDG_RUNTIME_DIR", &base)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_CONFIG_HOME", &config)
            .output()
            .expect("运行 hmp 失败");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // 建歌单 → 加本地曲目 → 播放。
    let created = run(&["playlist", "create", "集成测试歌单"]);
    assert!(created.contains("歌单"), "创建歌单失败: {created}");
    let pid: i64 = created
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .expect("解析歌单 id 失败");
    run(&[
        "playlist",
        "add",
        &pid.to_string(),
        &format!("local:{}", wav.display()),
    ]);
    let out = run(&["play", &format!("playlist:local:{pid}")]);
    assert!(out.contains("已开始播放"), "play 应报告开始播放: {out}");
    // 轮询 status：进入播放（本地源免凭证，离线可播）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut saw_playing = false;
    while std::time::Instant::now() < deadline {
        let st = run(&["status"]);
        if st.contains("播放中") || st.contains("Playing") {
            saw_playing = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    assert!(saw_playing, "本地歌单应开始播放");

    run(&["quit"]);
    wait_until(
        || !socket.exists(),
        Duration::from_secs(5),
        "quit 后 socket 清理",
    );
    let status = daemon.wait().expect("等待 daemon 退出失败");
    assert!(status.success(), "daemon 退出码异常: {status:?}");
    let _ = std::fs::remove_dir_all(&base);
}

/// 打磨：`hmp serve --background --sink fakesink` 应正常启动（fakesink 是
/// 有效 GStreamer sink，无默认音频输出的环境也能跑）并优雅退出。
#[test]
#[ignore = "需要真实 GStreamer 环境（真机验收项）"]
fn serve_with_explicit_sink_starts() {
    let base = std::env::temp_dir().join(format!("hmp-cli-sink-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let socket = base.join("hmp.sock");

    let mut daemon = Command::new(hmp_bin())
        .args(["serve", "--background", "--sink", "fakesink"])
        .env("XDG_RUNTIME_DIR", &base)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hmp serve --sink fakesink 失败");
    // fakesink 是有效元素：GstDriver 应成功创建并启动 daemon。
    assert!(
        wait_for_socket(&socket, Duration::from_secs(15)),
        "daemon socket 未就绪（--sink fakesink 启动失败?）"
    );
    let out = Command::new(hmp_bin())
        .args(["quit"])
        .env("XDG_RUNTIME_DIR", &base)
        .output()
        .expect("hmp quit 失败");
    assert!(out.status.success(), "hmp quit 失败: {out:?}");
    wait_until(
        || !socket.exists(),
        Duration::from_secs(5),
        "quit 后 socket 清理",
    );
    let status = daemon.wait().expect("等待 daemon 退出失败");
    assert!(status.success(), "daemon 退出码异常: {status:?}");
    let _ = std::fs::remove_dir_all(&base);
}

/// 最小 wav 文件（8kHz 单声道 1 秒，GStreamer 可直接播放）。
fn write_wav(path: &std::path::Path) {
    let sample_rate = 8000u32;
    let n = sample_rate as usize;
    let mut data = Vec::with_capacity(44 + n * 2);
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&((36 + n * 2) as u32).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&sample_rate.to_le_bytes());
    data.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&((n * 2) as u32).to_le_bytes());
    for _ in 0..n {
        data.extend_from_slice(&0u16.to_le_bytes()); // 静音（避免噪音）
    }
    std::fs::write(path, data).unwrap();
}
