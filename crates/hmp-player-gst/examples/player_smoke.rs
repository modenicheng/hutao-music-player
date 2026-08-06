//! 播放器冒烟：本地 WAV 播放、暂停、Seek、音量、停止（无音频设备环境用 fakeaudiosink）。
//!
//! 运行：`cargo run -p hmp-player-gst --example player_smoke`

use std::time::Duration;

use hmp_core::{AudioQuality, PlaybackStatus, Track, TrackId};
use hmp_player_gst::{LoadRequest, PlayerCore};

/// 生成 2 秒 440Hz 正弦波 AIFF（mono / 44100Hz / s16le）。
///
/// 用 AIFF 而非 WAV：测试环境仅装 gst-plugins-good 的 aiff 解析器，
/// 缺少 wavparse。
fn write_test_aiff(path: &std::path::Path) -> std::io::Result<()> {
    const RATE: u32 = 44100;
    const SECS: u32 = 2;
    const CH: u16 = 1;
    const BITS: u16 = 16;
    let samples = (RATE * SECS) as usize;
    let mut pcm = Vec::with_capacity(samples * 2);
    for i in 0..samples {
        let t = i as f64 / RATE as f64;
        let v = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 0.3;
        pcm.extend_from_slice(&((v * i16::MAX as f64) as i16).to_le_bytes());
    }
    // 80 位 IEEE extended float（采样率）
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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    let aiff_path = std::env::temp_dir().join("hmp-player-smoke.aiff");
    write_test_aiff(&aiff_path).expect("write test aiff");
    let uri = format!("file://{}", aiff_path.display());

    let core = PlayerCore::new_with_sink(Some("fakeaudiosink")).expect("init player");
    let state_rx = core.subscribe_state();
    let mut events_rx = core.subscribe_events();

    let track =
        Track::new(TrackId::new("test-wav"), "测试音频").with_quality(AudioQuality::Mp3_128);
    core.load(LoadRequest {
        track,
        uri,
        quality: AudioQuality::Mp3_128,
    });
    core.play();

    // 等待进入 Playing
    let mut got_playing = false;
    let mut got_duration = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let s = state_rx.borrow().clone();
        if s.status == PlaybackStatus::Playing {
            got_playing = true;
        }
        if s.duration.is_some() {
            got_duration = true;
        }
        if got_playing && got_duration {
            break;
        }
    }
    assert!(
        got_playing,
        "should reach Playing, state={:?}",
        state_rx.borrow().status
    );
    assert!(got_duration, "duration should be published");
    let duration = state_rx.borrow().duration.unwrap();
    println!("playing, duration={:?}", duration);

    // Seek 到 1 秒
    core.seek(Duration::from_secs(1));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let pos = state_rx.borrow().position;
    println!("after seek position={:?}", pos);
    assert!(
        pos >= Duration::from_millis(500),
        "seek should move position"
    );

    // 暂停
    core.pause();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(state_rx.borrow().status, PlaybackStatus::Paused);
    println!("paused ok");

    // 音量
    core.set_volume(0.42);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let vol = state_rx.borrow().volume;
    assert!(
        (vol - 0.42).abs() < 1e-9,
        "volume should be 0.42, got {vol}"
    );
    println!("volume ok");

    // 停止
    core.stop();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(state_rx.borrow().status, PlaybackStatus::Stopped);
    println!("stopped ok");

    core.shutdown();
    println!("player smoke PASSED");
    let _ = events_rx.try_recv();
}

trait TestExt {
    fn with_quality(self, q: AudioQuality) -> Self;
}
impl TestExt for Track {
    fn with_quality(mut self, q: AudioQuality) -> Self {
        self.qualities.push(q);
        self
    }
}
