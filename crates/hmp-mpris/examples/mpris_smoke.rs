//! MPRIS 冒烟：注册服务 → 播放测试音源 → playerctl 查询/控制。
//!
//! 运行：`cargo run -p hmp-mpris --example mpris_smoke`
//! 依赖：dbus session + playerctl（本环境已具备）。

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use hmp_core::{AudioQuality, Track, TrackId};
use hmp_mpris::MprisService;
use hmp_player_gst::{LoadRequest, PlayerCore};

fn write_test_aiff(path: &std::path::Path) -> std::io::Result<()> {
    const RATE: u32 = 44100;
    const SECS: u32 = 5;
    const CH: u16 = 1;
    const BITS: u16 = 16;
    let samples = (RATE * SECS) as usize;
    let mut pcm = Vec::with_capacity(samples * 2);
    for i in 0..samples {
        let t = i as f64 / RATE as f64;
        let v = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 0.3;
        pcm.extend_from_slice(&((v * i16::MAX as f64) as i16).to_le_bytes());
    }
    fn ext80(x: f64) -> [u8; 10] {
        let mut v = x.abs();
        let mut exp = 0i32;
        while v >= 2.0 {
            v /= 2.0;
            exp += 1;
        }
        while v < 1.0 {
            v *= 2.0;
            exp -= 1;
        }
        let frac = ((v - 1.0) * (1u64 << 63) as f64).round() as u64;
        let mut out = [0u8; 10];
        out[..2].copy_from_slice(&((exp + 16383) as u16).to_be_bytes());
        out[2..].copy_from_slice(&frac.to_be_bytes());
        out
    }
    let comm = CH
        .to_be_bytes()
        .into_iter()
        .chain((samples as u32).to_be_bytes())
        .chain(BITS.to_be_bytes())
        .chain(ext80(RATE as f64))
        .collect::<Vec<_>>();
    let ssnd = [0u32.to_be_bytes(), 0u32.to_be_bytes()]
        .into_iter()
        .flatten()
        .chain(pcm)
        .collect::<Vec<_>>();
    let mut out = Vec::new();
    out.extend_from_slice(b"FORM");
    out.extend_from_slice(&((4 + 8 + comm.len() + 8 + ssnd.len()) as u32).to_be_bytes());
    out.extend_from_slice(b"AIFF");
    out.extend_from_slice(b"COMM");
    out.extend_from_slice(&(comm.len() as u32).to_be_bytes());
    out.extend_from_slice(&comm);
    out.extend_from_slice(b"SSND");
    out.extend_from_slice(&(ssnd.len() as u32).to_be_bytes());
    out.extend_from_slice(&ssnd);
    std::fs::write(path, out)
}

fn playerctl(args: &[&str]) -> String {
    let out = Command::new("playerctl")
        .args(args)
        .output()
        .expect("run playerctl");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    // 播放器（fakeaudiosink：无音频设备环境）
    let core = Arc::new(PlayerCore::new_with_sink(Some("fakeaudiosink")).expect("player"));
    let state_rx = core.subscribe_state();

    // MPRIS 服务
    let mpris = MprisService::start(core.command_sender(), state_rx)
        .await
        .expect("start mpris");
    println!("MPRIS service started on session bus");
    println!("bus name: org.mpris.MediaPlayer2.hmp");

    // 播放测试音源
    let aiff = std::env::temp_dir().join("hmp-mpris-smoke.aiff");
    write_test_aiff(&aiff).expect("write aiff");
    let track = Track {
        id: TrackId::new("test-mpris-1"),
        title: "MPRIS 测试曲目".into(),
        artists: vec![hmp_core::ArtistRef {
            id: hmp_core::ArtistId::new("artist-1"),
            name: "测试歌手".into(),
        }],
        album: Some(hmp_core::AlbumRef {
            id: hmp_core::AlbumId::new("album-1"),
            name: "测试专辑".into(),
        }),
        duration: Some(Duration::from_secs(5)),
        cover: None,
        url: None,
        available_qualities: vec![AudioQuality::Mp3_128],
    };
    core.load(LoadRequest {
        uri: format!("file://{}", aiff.display()),
        track,
        quality: AudioQuality::Mp3_128,
        load_gen: 0,
    });
    core.play();

    // 等待 Playing
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // ---- zbus 自检（对比 playerctl）----
    let props = zbus::fdo::PropertiesProxy::builder(mpris.connection())
        .destination("org.mpris.MediaPlayer2.hmp")
        .expect("dest")
        .path("/org/mpris/MediaPlayer2")
        .expect("path")
        .build()
        .await
        .expect("props proxy");
    match props
        .get(
            "org.mpris.MediaPlayer2.Player".try_into().expect("iface"),
            "PlaybackStatus",
        )
        .await
    {
        Ok(v) => println!("zbus PlaybackStatus: {v:?}"),
        Err(e) => println!("zbus get failed: {e}"),
    }

    // ---- playerctl 查询 ----
    let status = playerctl(&["-p", "hmp", "status"]);
    println!("status: {status}");
    assert_eq!(status, "Playing", "should be playing");

    let title = playerctl(&["-p", "hmp", "metadata", "xesam:title"]);
    println!("title: {title}");
    assert_eq!(title, "MPRIS 测试曲目");

    let artist = playerctl(&["-p", "hmp", "metadata", "xesam:artist"]);
    println!("artist: {artist}");
    assert!(artist.contains("测试歌手"));

    // ---- playerctl 控制：暂停 ----
    playerctl(&["-p", "hmp", "pause"]);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let status = playerctl(&["-p", "hmp", "status"]);
    println!("after pause: {status}");
    assert_eq!(status, "Paused");

    // ---- 播放 ----
    playerctl(&["-p", "hmp", "play"]);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let status = playerctl(&["-p", "hmp", "status"]);
    println!("after play: {status}");
    assert_eq!(status, "Playing");

    // ---- 位置 ----
    playerctl(&["-p", "hmp", "position", "2.0"]);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let pos = playerctl(&["-p", "hmp", "position"]);
    println!("position: {pos}");
    let pos_s: f64 = pos.parse().unwrap_or(0.0);
    assert!(pos_s >= 1.5, "position should have moved, got {pos_s}");

    // ---- 音量 ----
    playerctl(&["-p", "hmp", "volume", "0.5"]);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let vol = playerctl(&["-p", "hmp", "volume"]);
    println!("volume: {vol}");
    let vol_f: f64 = vol.parse().unwrap_or(0.0);
    assert!(
        (vol_f - 0.5).abs() < 0.01,
        "volume should be 0.5, got {vol_f}"
    );

    core.shutdown();
    println!("MPRIS smoke PASSED");
}
