//! 端到端冒烟：Play → 详情 → 音质回退 → 取流 → 播放 → Ended → 自动续播。
//!
//! 覆盖 spec §8 后台播放链路的两段接缝：
//!
//! 1. [`resolve_track_falls_back_to_plain_via_mock_api`]：真实
//!    [`QqSourceResolver`] + wiremock QQ API（曲目详情 + 取流）+ 文件凭证
//!    后端，验证「详情解析 → 加密音质全部失败 → 明文音质成功」的完整回退链
//!    与 CDN URI 契约（无 GStreamer，完全离线）。
//! 2. [`play_then_end_advances_queue_with_gst`]：真实 [`GstDriver`]
//!    （fakeaudiosink，headless）+ 本地生成的 1s wav，验证「Play → Playing →
//!    真实 EOS → 自动续播下一首 → 队列播完」；队列裁决逻辑由引擎单测
//!    （engine.rs）覆盖，本测试是真实 GStreamer 冒烟。
//!
//! 凭证隔离通过环境变量：`HMP_CREDENTIAL_BACKEND=file` + `XDG_CONFIG_HOME`
//! 指向临时目录（`FileStore` 落在 `$XDG_CONFIG_HOME/hmp/credential.json`，
//! 见 hmp-storage xdg.rs）。
//!
//! 已知约束：daemon 取流拼接的 CDN 域名固定为
//! `https://isure.stream.qqmusic.qq.com/<purl>`（player.rs），因此播放阶段
//! 无法把音频指向 wiremock —— 明文/解密取流由测试 1 以响应契约方式验证，
//! 实际音频播放由测试 2 以本地 wav 覆盖。

use std::collections::HashMap;
use std::ffi::OsString;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use hmp_core::{AudioQuality, DaemonState, PlayRequest, PlaybackStatus, Request, Track, TrackId};
use hmp_daemon::engine::PlaybackEngine;
use hmp_daemon::player::{
    EngineError, GstDriver, PlaybackDriver, QqSourceResolver, ResolvedTrack, SourceResolver,
};
use hmp_qqmusic_api::{ClientConfig, Credential, QqMusicClient};
use hmp_storage::credential::{Store, store_from_env};
use hmp_storage::xdg::config_dir;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 测试曲目（mid 非纯数字 → 详情请求走 `song_mid` 参数）。
const TRACK_MID: &str = "003xYzAbTestMid";
/// 媒体文件 mid（详情 file.media_mid，取流文件名以它拼 `M500<media_mid>.mp3`）。
const TRACK_MEDIA_MID: &str = "004wavMediaMid";
/// 队列中的两首本地 wav（GStreamer 冒烟用）。
const WAV_ID_1: &str = "localwav-1";
const WAV_ID_2: &str = "localwav-2";

// ── 测试 1：真实解析器 × wiremock QQ API ──────────────────────────────

/// 生成 1 秒 wav（8kHz 单声道 PCM16，440Hz 正弦，供 headless 播放测试）。
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
    for i in 0..n {
        let v = ((i as f64 * 2.0 * std::f64::consts::PI * 440.0 / f64::from(sample_rate)).sin()
            * 0.3
            * 32767.0) as i16;
        data.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, data).unwrap();
}

/// 恢复进程环境变量（edition 2024：`std::env::set_var` 为 unsafe）。
struct EnvGuard {
    backend: Option<OsString>,
    xdg_config: Option<OsString>,
}

/// 串行化修改环境变量的 resolve 测试（`Config::load` 读 `XDG_CONFIG_HOME`，
/// 与 `resolve_track_falls_back_to_plain_via_mock_api` 共享环境）。
static CONFIG_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl EnvGuard {
    /// 设置 file 凭证后端 + 临时配置目录，返回还原句柄。
    fn install(dir: &std::path::Path) -> Self {
        let backend = std::env::var_os("HMP_CREDENTIAL_BACKEND");
        let xdg_config = std::env::var_os("XDG_CONFIG_HOME");
        unsafe {
            std::env::set_var("HMP_CREDENTIAL_BACKEND", "file");
            std::env::set_var("XDG_CONFIG_HOME", dir);
        }
        Self {
            backend,
            xdg_config,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.backend {
                Some(v) => std::env::set_var("HMP_CREDENTIAL_BACKEND", v),
                None => std::env::remove_var("HMP_CREDENTIAL_BACKEND"),
            }
            match &self.xdg_config {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }
}

/// 构造指向 wiremock 的 QQ 客户端。
fn client_for(base_url: &str) -> QqMusicClient {
    let config = ClientConfig {
        base_url: base_url.to_owned(),
        ..Default::default()
    };
    QqMusicClient::with_config(config)
}

/// 解析请求体的 `req_0` 字段（wiremock 匹配闭包用）。
fn req0(req: &wiremock::Request) -> Value {
    serde_json::from_slice(&req.body).unwrap_or(json!({}))["req_0"].clone()
}

/// 取流响应：单个文件授权结果（明文成功）。
fn urls_ok(filename: &str, purl: &str) -> Value {
    json!({
        "code": 0,
        "req_0": {
            "code": 0,
            "data": {
                "expiration": 7200,
                "midurlinfo": [{
                    "songmid": TRACK_MID,
                    "filename": filename,
                    "purl": purl,
                    "result": 0,
                }]
            }
        }
    })
}

/// 取流响应：单个文件授权失败（加密音质/低品质不可用 → 触发回退）。
fn urls_fail() -> Value {
    json!({
        "code": 0,
        "req_0": {
            "code": 0,
            "data": {
                "expiration": 7200,
                "midurlinfo": [{
                    "songmid": TRACK_MID,
                    "filename": "",
                    "purl": "",
                    "result": 104003,
                }]
            }
        }
    })
}

/// 曲目详情响应（`track_info` 为上游字段，serde alias 到 `track`）。
fn detail_ok() -> Value {
    json!({
        "code": 0,
        "req_0": {
            "code": 0,
            "data": {
                "track_info": {
                    "id": 186016,
                    "mid": TRACK_MID,
                    "name": "开始懂了",
                    "singer": [{"id": 1001, "mid": "003abcSinger", "name": "孙燕姿"}],
                    "album": {
                        "id": 2002,
                        "mid": "003abcAlbum",
                        "name": "孙燕姿经典全纪录 主打精华版",
                        "pmid": "001coverPmid",
                    },
                    "interval": 270,
                    "file": { "media_mid": TRACK_MEDIA_MID },
                }
            }
        }
    })
}

/// 挂载 wiremock QQ API：详情成功；加密音质（GetEVkey）全部失败；
/// 明文音质 M800（Mp3_320）失败、M500（Mp3_128）成功。
///
/// 回退链（player.rs `CHAIN` + `quality_to_file_type`）：Master(AIM0) → HiRes(AIM0)
/// → Atmos(Q0M0) → Flac(F0M0) → Mp3_320(M800) → Mp3_128(M500)。前五个全部失败后，
/// 最后一个明文 M500 成功 → 最终音质为 Mp3_128。加密各档按文件名前缀分 mock，
/// 使测试能断言「Atmos 在 Flac 之前被尝试」（回退链回归，final review Finding 3）。
async fn mount_qq_mocks(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &wiremock::Request| req0(req)["method"] == json!("get_song_detail_yqq"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail_ok()))
        .mount(server)
        .await;

    // 加密取流（music.vkey.GetEVkey）：Master/HiRes 同档（AIM0）→ 失败；
    // Atmos（Q0M0）→ 失败；Flac（F0M0）→ 失败。
    for prefix in ["AIM0", "Q0M0", "F0M0"] {
        let prefix = prefix.to_owned();
        Mock::given(method("POST"))
            .and(path("/cgi-bin/musicu.fcg"))
            .and(move |req: &wiremock::Request| {
                let body = req0(req);
                let filename = body["param"]["filename"][0].as_str().unwrap_or("");
                body["module"] == json!("music.vkey.GetEVkey") && filename.starts_with(&prefix)
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(urls_fail()))
            .mount(server)
            .await;
    }

    // 明文取流（music.vkey.GetVkey）按文件名前缀区分音质
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &wiremock::Request| {
            let body = req0(req);
            let filename = body["param"]["filename"][0].as_str().unwrap_or("");
            filename.starts_with("M800")
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(urls_fail()))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &wiremock::Request| {
            let body = req0(req);
            let filename = body["param"]["filename"][0].as_str().unwrap_or("");
            filename.starts_with("M500")
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(urls_ok(
            &format!("M500{TRACK_MEDIA_MID}.mp3"),
            &format!("M500{TRACK_MEDIA_MID}.mp3?guid=abc&vkey=testvkey"),
        )))
        .mount(server)
        .await;
}

/// 真实 `QqSourceResolver` × wiremock：详情 + 回退 + 取流 → 可播放 URI。
///
/// 不触网、不依赖 GStreamer；验证 daemon 对 QQ API 响应的解析契约。
#[tokio::test]
async fn resolve_track_falls_back_to_plain_via_mock_api() {
    let _lock = CONFIG_ENV_LOCK.lock().unwrap();
    // 1) 凭证隔离：file 后端 + 临时 XDG_CONFIG_HOME（真实 daemon 的环境变量路径）
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::install(dir.path());
    let store: Store = store_from_env();
    assert!(matches!(
        hmp_storage::credential::BackendKind::from_env(),
        hmp_storage::credential::BackendKind::File
    ));
    store
        .save(&Credential {
            uin: "10001".into(),
            music_id: "10001".into(),
            music_key: "secret-key".into(),
            refresh_key: None,
            raw_cookie: String::new(),
            str_musicid: "10001".into(),
            ..Default::default()
        })
        .unwrap();
    // 凭证落盘位置 = $XDG_CONFIG_HOME/hmp/credential.json
    assert!(
        config_dir().join("credential.json").exists(),
        "file 凭证应落在 $XDG_CONFIG_HOME/hmp/credential.json"
    );

    // 2) wiremock QQ API
    let server = MockServer::start().await;
    mount_qq_mocks(&server).await;

    // 3) 真实解析器（mock 客户端 + 共享凭证）
    let resolver = QqSourceResolver::new(client_for(&server.uri()), store);
    assert!(resolver.has_credential(), "凭证已保存应可读取");

    // 4) 单曲源解析为 [mid]
    let ids = resolver
        .resolve_source_ids(&PlayRequest::Track(TrackId::new(TRACK_MID)))
        .await
        .unwrap();
    assert_eq!(ids, vec![TrackId::new(TRACK_MID)]);

    // 5) 曲目解析：详情 → 回退（加密全失败）→ 明文 M500 成功
    let resolved = resolver
        .resolve_track(&TrackId::new(TRACK_MID))
        .await
        .expect("回退到明文音质后应成功解析");

    // CDN URI 契约（daemon 拼接固定域名 + purl）
    assert_eq!(
        resolved.uri,
        format!(
            "https://isure.stream.qqmusic.qq.com/M500{TRACK_MEDIA_MID}.mp3?guid=abc&vkey=testvkey"
        )
    );
    // 明文音质无需解密 guard（media = None → GStreamer 直连 CDN）
    assert!(resolved.media.is_none(), "明文路径不应有解密代理 guard");

    // 元数据（歌手/专辑/封面/时长）来自详情
    assert_eq!(resolved.track.id, TrackId::new(TRACK_MID));
    assert_eq!(resolved.track.title, "开始懂了");
    assert_eq!(resolved.track.artists.len(), 1);
    assert_eq!(resolved.track.artists[0].name, "孙燕姿");
    assert_eq!(
        resolved.track.album.as_ref().map(|a| a.name.as_str()),
        Some("孙燕姿经典全纪录 主打精华版")
    );
    assert_eq!(resolved.track.duration, Some(Duration::from_secs(270)));
    assert!(resolved.track.cover.is_some());
    assert_eq!(
        resolved.track.available_qualities,
        vec![AudioQuality::Mp3_128],
        "回退链最终应落在 Mp3_128（M500）"
    );
    assert_eq!(resolved.track.url.as_deref(), Some(resolved.uri.as_str()));

    // 回退链顺序回归（final review Finding 3）：加密档须按
    // Master/HiRes(AIM0) → Atmos(Q0M0) → Flac(F0M0) 顺序依次尝试。
    // 若链退化（漏 Atmos），Q0M0 请求不会出现，本断言失败。
    let evkey_prefixes: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| req0(r)["module"] == json!("music.vkey.GetEVkey"))
        .filter_map(|r| {
            let f = req0(r)["param"]["filename"][0]
                .as_str()
                .unwrap_or("")
                .to_owned();
            (f.len() >= 4).then(|| f[..4].to_owned())
        })
        .collect();
    let prefixes: Vec<&str> = evkey_prefixes.iter().map(|s| s.as_str()).collect();
    assert!(
        prefixes.contains(&"Q0M0"),
        "回退链应尝试 Atmos（Q0M0），实际 GetEVkey 序列: {prefixes:?}"
    );
    let atmos = prefixes.iter().position(|p| *p == "Q0M0").unwrap();
    let flac = prefixes.iter().position(|p| *p == "F0M0").unwrap();
    assert!(
        atmos < flac,
        "Atmos（Q0M0）应在 Flac（F0M0）之前尝试，实际序列: {prefixes:?}"
    );
}

// ── 测试 2：真实 GStreamer × fakesink × 本地 wav ──────────────────────

/// 把本地 wav 当播放源的解析器：模拟歌单 `PlayRequest::Playlist` →
/// [t1, t2]，每首解析为 `file://` URI（真实播放本地音频，产生真实 EOS）。
struct LocalWavResolver {
    playlist: Vec<TrackId>,
    wavs: HashMap<TrackId, String>,
}

impl LocalWavResolver {
    fn new(wavs: Vec<(TrackId, String)>) -> Self {
        let playlist = wavs.iter().map(|(id, _)| id.clone()).collect();
        let wavs = wavs.into_iter().collect();
        Self { playlist, wavs }
    }
}

impl SourceResolver for LocalWavResolver {
    fn resolve_source_ids(
        &self,
        src: &hmp_core::PlayRequest,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<TrackId>, EngineError>> + Send + '_>>
    {
        let ids = match src {
            hmp_core::PlayRequest::Playlist(_) => self.playlist.clone(),
            hmp_core::PlayRequest::Track(id) => vec![id.clone()],
            hmp_core::PlayRequest::Album(_) => Vec::new(),
        };
        Box::pin(async move { Ok(ids) })
    }

    fn resolve_track(
        &self,
        track_id: &TrackId,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>>
    {
        let id = track_id.clone();
        let wav = self.wavs.get(track_id).cloned();
        Box::pin(async move {
            let wav = wav.ok_or(EngineError::TrackNotFound)?;
            Ok(ResolvedTrack {
                track: Track {
                    id: id.clone(),
                    title: format!("本地wav-{id}"),
                    artists: vec![],
                    album: None,
                    duration: Some(Duration::from_secs(1)),
                    cover: None,
                    url: Some(format!("file://{wav}")),
                    available_qualities: vec![AudioQuality::Mp3_128],
                },
                uri: format!("file://{wav}"),
                media: None,
                quality: AudioQuality::Mp3_128,
            })
        })
    }
}

/// 轮询复合状态直到满足条件（超时 panic）。
async fn wait_state(
    mut rx: tokio::sync::watch::Receiver<DaemonState>,
    timeout: Duration,
    cond: impl FnMut(&DaemonState) -> bool,
) -> DaemonState {
    tokio::time::timeout(timeout, async {
        let mut cond = cond;
        loop {
            let st = rx.borrow().clone();
            if cond(&st) {
                return st;
            }
            if rx.changed().await.is_err() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    })
    .await
    .expect("等待 daemon 状态超时")
}

/// 等待下一个 `PlaybackEnded` 事件（真实 EOS 的直接证据；
/// 每次调用从当前广播游标起等待一次）。
async fn wait_next_ended(
    events: &mut tokio::sync::broadcast::Receiver<hmp_player_gst::PlayerEvent>,
    timeout: Duration,
) {
    tokio::time::timeout(timeout, async {
        loop {
            match events.recv().await {
                Ok(hmp_player_gst::PlayerEvent::PlaybackEnded) => return,
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    })
    .await
    .expect("等待 PlaybackEnded 事件超时")
}

/// 引擎 Play → Playing → 真实 EOS → 自动续播下一首 → 队列播完。
///
/// 非 `#[ignore]`：与 hmp-player-gst 既有测试一致，使用 `fakeaudiosink`
/// headless 运行（GStreamer 为本仓库 workspace 测试的硬依赖）。
///
/// 不用 `fakesink`：gstreamer-player 在 fakesink 下 EOS/续播时序不稳定
/// （首曲可能不触发 EOS、紧接 EOS 的换曲可能停在 Stopped）；
/// `fakeaudiosink` 是仓库 headless 约定（hmp-player-gst 既有测试），
/// 顺序加载与 EOS 行为确定。
/// 固定音质策略（`hmp quality flac`）：回退链从 FLAC 起，不再尝试 Master/HiRes/Atmos。
/// 断言：GetEVkey 序列只含 F0M0（Q0M0/AIM0 不出现）→ 加密失败后明文 M500 兜底。
#[tokio::test]
async fn resolve_track_respects_fixed_quality_config() {
    let _lock = CONFIG_ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::install(dir.path());
    // 写配置：固定 FLAC + 允许回退。
    let cfg_dir = dir.path().join("hmp");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
        "[quality]\nmode = \"flac\"\nfallback = true\n",
    )
    .unwrap();

    let store: Store = store_from_env();
    store
        .save(&Credential {
            uin: "10001".into(),
            music_id: "10001".into(),
            music_key: "secret-key".into(),
            refresh_key: None,
            raw_cookie: String::new(),
            str_musicid: "10001".into(),
            ..Default::default()
        })
        .unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &wiremock::Request| req0(req)["method"] == json!("get_song_detail_yqq"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail_ok()))
        .mount(&server)
        .await;
    // FLAC（F0M0，加密）失败 → 回退 M800 失败 → M500 明文成功。
    for (prefix, body) in [("F0M0", urls_fail()), ("M800", urls_fail())] {
        let prefix = prefix.to_owned();
        Mock::given(method("POST"))
            .and(path("/cgi-bin/musicu.fcg"))
            .and(move |req: &wiremock::Request| {
                let body = req0(req);
                let filename = body["param"]["filename"][0].as_str().unwrap_or("");
                body["module"] == json!("music.vkey.GetEVkey") && filename.starts_with(&prefix)
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &wiremock::Request| {
            let body = req0(req);
            let filename = body["param"]["filename"][0].as_str().unwrap_or("");
            filename.starts_with("M500")
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(urls_ok(
            &format!("M500{TRACK_MEDIA_MID}.mp3"),
            &format!("M500{TRACK_MEDIA_MID}.mp3?guid=abc&vkey=testvkey"),
        )))
        .mount(&server)
        .await;

    let resolver = QqSourceResolver::new(client_for(&server.uri()), store);
    let resolved = resolver
        .resolve_track(&TrackId::new(TRACK_MID))
        .await
        .unwrap();
    assert_eq!(resolved.quality, AudioQuality::Mp3_128); // 固定 FLAC 失败 → 回退到 128

    // 链从 FLAC 开始：Master(AIM0)/HiRes(AIM0)/Atmos(Q0M0) 从未被请求。
    let prefixes: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| req0(r)["module"] == json!("music.vkey.GetEVkey"))
        .filter_map(|r| {
            req0(r)["param"]["filename"][0]
                .as_str()
                .map(|s| s[..4].to_string())
        })
        .collect();
    assert_eq!(
        prefixes,
        vec!["F0M0"],
        "固定 FLAC 只应尝试 F0M0，实际: {prefixes:?}"
    );
}

#[tokio::test]
async fn play_then_end_advances_queue_with_gst() {
    // 1) 本地 1s wav（两首，验证续播）
    let dir = tempfile::tempdir().unwrap();
    let wav1 = dir.path().join("t1.wav");
    let wav2 = dir.path().join("t2.wav");
    write_wav(&wav1);
    write_wav(&wav2);

    // 2) 真实 GStreamer 驱动（fakeaudiosink，headless）
    let driver: Arc<dyn PlaybackDriver> = Arc::new(
        GstDriver::new(Some("fakeaudiosink")).expect("GStreamer 初始化失败（fakeaudiosink）"),
    );

    // 3) 本地 wav 解析器（模拟歌单队列 [t1, t2]）
    let resolver: Arc<dyn SourceResolver> = Arc::new(LocalWavResolver::new(vec![
        (TrackId::new(WAV_ID_1), wav1.display().to_string()),
        (TrackId::new(WAV_ID_2), wav2.display().to_string()),
    ]));

    // 4) 启动引擎并播放歌单；额外订阅驱动事件（直接观察真实 EOS）
    let handle = PlaybackEngine::start(driver.clone(), resolver, Arc::new(|| true));
    let mut events = driver.subscribe_events();
    handle
        .cmd(Request::Play(PlayRequest::Playlist(
            hmp_core::PlaylistId::new("e2e-list"),
        )))
        .await
        .unwrap();

    // 5) 首曲进入 Playing（当前 = 队列 0，队列 2 首）
    let st = wait_state(handle.state_rx.clone(), Duration::from_secs(10), |st| {
        st.queue.tracks.len() == 2
            && st.queue.current == Some(0)
            && st.playback.status == PlaybackStatus::Playing
    })
    .await;
    assert_eq!(st.queue.tracks[0], TrackId::new(WAV_ID_1));
    assert_eq!(
        st.playback
            .current
            .as_ref()
            .map(|t| t.id == TrackId::new(WAV_ID_1)),
        Some(true),
        "首曲应加载 t1"
    );

    // 6) 首曲真实 EOS（1s wav 播完）→ 引擎自动续播 → 第二首已加载
    wait_next_ended(&mut events, Duration::from_secs(15)).await;
    let advanced = wait_state(handle.state_rx.clone(), Duration::from_secs(15), |st| {
        st.queue.current == Some(1)
            && st
                .playback
                .current
                .as_ref()
                .map(|t| t.id == TrackId::new(WAV_ID_2))
                == Some(true)
    })
    .await;
    assert!(
        matches!(
            advanced.playback.status,
            PlaybackStatus::Playing | PlaybackStatus::Ended | PlaybackStatus::Stopped
        ),
        "续播后状态应为 Playing（或已到第二次结束），实际 {:?}",
        advanced.playback.status
    );

    // 7) 队列播完：第二次 EOS 后停在最后一首（current 保持 1）。
    //    gstreamer-player 在 EOS 后自行停管线（state-changed STOPPED），
    //    会覆盖核心发布的 Ended —— 故收尾状态接受 Ended | Stopped。
    wait_next_ended(&mut events, Duration::from_secs(15)).await;
    let st = wait_state(handle.state_rx.clone(), Duration::from_secs(10), |st| {
        (st.playback.status == PlaybackStatus::Ended
            || st.playback.status == PlaybackStatus::Stopped)
            && st.queue.current == Some(1)
            && st
                .playback
                .current
                .as_ref()
                .map(|t| t.id == TrackId::new(WAV_ID_2))
                == Some(true)
    })
    .await;
    assert_ne!(
        st.playback.status,
        PlaybackStatus::Error,
        "整段播放不得出错"
    );

    // 8) 优雅退出（引擎终止 → 驱动 shutdown；sticky watch，Finding 7）
    handle.cmd(Request::Quit).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut term = handle.terminated.clone();
        if *term.borrow() {
            return;
        }
        let _ = term.changed().await;
        assert!(*term.borrow());
    })
    .await
    .expect("Quit 后引擎应终止");
}
