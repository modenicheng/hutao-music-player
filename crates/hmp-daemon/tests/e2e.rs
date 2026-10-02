//! 端到端冒烟：Play → 详情 → 音质回退 → 取流 → 播放 → Ended → 自动续播。
//!
//! 覆盖 spec §8 后台播放链路的两段接缝：
//!
//! 1. [`resolve_track_falls_back_to_plain_via_mock_api`]：真实
//!    [`QqSourceResolver`] + wiremock QQ API（曲目详情 + 取流）+ 文件凭证
//!    后端，验证「详情解析 → 加密音质全部失败 → 明文音质成功」的完整回退链
//!    与 CDN URI 契约（无音频设备，完全离线）。
//! 2. [`play_then_end_advances_queue_with_rodio`]：真实
//!    [`RodioDriver`] + 本地生成的 1s wav，验证「Play → Playing →
//!    真实 EOS → 自动续播下一首 → 队列播完」；队列裁决逻辑由引擎单测
//!    （engine.rs）覆盖，本测试是真实音频输出冒烟。
//!
//! 凭证隔离通过环境变量：`HMP_CREDENTIAL_BACKEND=file` + `XDG_CONFIG_HOME`
//! 指向临时目录（`FileStore` 落在 `$XDG_CONFIG_HOME/hmp/credential.json`，
//! 见 hmp-storage xdg.rs）。
//!
//! 已知约束：daemon 取流拼接的 CDN 域名固定为
//! `https://isure.stream.qqmusic.qq.com/<purl>`（player.rs），无法把播放阶段
//! 的音频指向 wiremock。新链路（加密/明文统一 `hmp_media::prepare_media`
//! 进程内直连）下的测试分工：
//!
//! 1. 音质回退链与「明文音质成功」经 wiremock QQ API + **预置播放缓存**
//!    验证（`cached_playable_uri` 命中 → `file://` + 无进程内源，零 CDN）；
//!    命中键由 daemon 拼接的 CDN url path 派生，命中本身即验证 URL 拼接契约。
//! 2. 流式 `PreparedMedia` 契约（CDN url + `source`）由
//!    [`prepared_media_contract_is_cdn_url_plus_stream_source`] 直接以
//!    wiremock 充当 Range CDN 验证。
//! 3. 实际音频播放由真机 `#[ignore]` 测试以本地 wav 覆盖。

use std::collections::HashMap;
use std::ffi::OsString;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use hmp_core::{AudioQuality, DaemonState, PlayRequest, PlaybackStatus, Request, Track, TrackId};
use hmp_daemon::engine::PlaybackEngine;
use hmp_daemon::player::{
    EngineError, PlaybackDriver, QqSourceResolver, ResolvedTrack, RodioDriver, SourceResolver,
};
use hmp_qqmusic_api::{ClientConfig, Credential, QqMusicClient};
use hmp_storage::credential::{Store, store_from_env};
use hmp_storage::xdg::config_dir;
use serde_json::{Value, json};
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 测试曲目（mid 非纯数字 → 详情请求走 `song_mid` 参数）。
const TRACK_MID: &str = "003xYzAbTestMid";
/// 媒体文件 mid（详情 file.media_mid，取流文件名以它拼 `M500<media_mid>.mp3`）。
const TRACK_MEDIA_MID: &str = "004wavMediaMid";
/// 队列中的两首本地 wav（真实音频冒烟用）。
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
    xdg_cache: Option<OsString>,
}

/// 串行化修改环境变量的 resolve 测试（`Config::load` 读 `XDG_CONFIG_HOME`，
/// 与 `resolve_track_falls_back_to_plain_via_mock_api` 共享环境）。
static CONFIG_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl EnvGuard {
    /// 设置 file 凭证后端 + 临时配置/缓存目录，返回还原句柄。
    /// 缓存目录隔离：resolve 的 `cached_playable_uri` 与流式 tee 落
    /// `XDG_CACHE_HOME`（Windows 亦尊重该覆盖，见 hmp-storage xdg.rs）。
    fn install(dir: &std::path::Path) -> Self {
        let backend = std::env::var_os("HMP_CREDENTIAL_BACKEND");
        let xdg_config = std::env::var_os("XDG_CONFIG_HOME");
        let xdg_cache = std::env::var_os("XDG_CACHE_HOME");
        unsafe {
            std::env::set_var("HMP_CREDENTIAL_BACKEND", "file");
            std::env::set_var("XDG_CONFIG_HOME", dir);
            std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
        }
        Self {
            backend,
            xdg_config,
            xdg_cache,
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
            match &self.xdg_cache {
                Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
                None => std::env::remove_var("XDG_CACHE_HOME"),
            }
        }
    }
}

/// 预置播放缓存命中：按稳定键（URL path | ekey）在
/// `$XDG_CACHE_HOME/hmp/decrypted` 写入合法魔数音频文件。resolve 的
/// `cached_playable_uri` 即命中（零 CDN 网络路径）——wiremock 场景无法
/// 触达真实 CDN（域名固定），借此走通「明文音质成功」路径；键由 daemon
/// 拼接的 CDN url path 派生，命中本身即验证 URL 拼接契约。
fn seed_playback_cache(cdn_url: &str, ekey: &str, bytes: &[u8]) {
    let key = hmp_media::cache::cache_key(cdn_url, ekey);
    let root = hmp_storage::cache_dir().join("decrypted");
    std::fs::create_dir_all(&root).unwrap();
    let ext = hmp_media::cache::extension_from_magic(bytes).expect("种子文件须有可识别魔数");
    std::fs::write(root.join(format!("{key}.{ext}")), bytes).unwrap();
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
    urls_fail_with(104003)
}

/// 取流响应：单个文件授权失败，业务码可指定（鉴权分类回归用）。
fn urls_fail_with(result: i64) -> Value {
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
                    "result": result,
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
/// 不触网、不依赖 Rodio；验证 daemon 对 QQ API 响应的解析契约。
#[tokio::test]
async fn resolve_track_falls_back_to_plain_via_mock_api() {
    let _lock = CONFIG_ENV_LOCK.lock().await;
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
    assert_eq!(
        ids.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        vec![TrackId::new(TRACK_MID)]
    );

    // 5) 曲目解析：详情 → 回退（加密全失败）→ 明文 M500 成功。
    //    预置播放缓存命中（零 CDN——域名固定无法指向 wiremock，见模块注释；
    //    键由 daemon 拼接的 CDN url path 派生，命中即验证拼接契约）。
    let cdn_url = format!(
        "https://isure.stream.qqmusic.qq.com/M500{TRACK_MEDIA_MID}.mp3?guid=abc&vkey=testvkey"
    );
    seed_playback_cache(&cdn_url, "", b"ID3\x00\x00\x00\x00hmp-e2e-seed");
    let resolved = resolver
        .resolve_track(&TrackId::new(TRACK_MID))
        .await
        .expect("回退到明文音质后应成功解析");

    // 播放缓存命中契约：file:// 本地 URI + 无进程内源（media = None）。
    assert!(
        resolved.uri.starts_with("file://"),
        "缓存命中应返回 file:// URI，实际 {}",
        resolved.uri
    );
    assert!(
        resolved.media.is_none(),
        "缓存命中路径不应携带进程内源（直接播本地文件）"
    );

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

// ── 测试 2：真实 Rodio × 本地 wav ─────────────────────────────────────

/// 把本地 wav 当播放源的解析器：模拟歌单 `PlayRequest::Playlist` →
/// [t1, t2]，每首解析为 `file://` URI（真实播放本地音频，产生真实 EOS）。
#[derive(Debug)]
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
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>,
    > {
        let stubs = match src {
            hmp_core::PlayRequest::Playlist(_) => self
                .playlist
                .iter()
                .map(|id| hmp_core::TrackStub {
                    id: id.clone(),
                    title: id.to_string(),
                    artists: Vec::new(),
                    album: None,
                    duration_ms: None,
                })
                .collect(),
            hmp_core::PlayRequest::Track(id) => vec![hmp_core::TrackStub {
                id: id.clone(),
                title: id.to_string(),
                artists: Vec::new(),
                album: None,
                duration_ms: None,
            }],
            hmp_core::PlayRequest::Album(_) => Vec::new(),
            hmp_core::PlayRequest::Local(_) => Vec::new(),
            hmp_core::PlayRequest::LibraryPlaylist(_) => Vec::new(),
        };
        Box::pin(async move { Ok(stubs) })
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
                uri: url::Url::from_file_path(&wav)
                    .map_err(|()| EngineError::Internal("无效本地路径".into()))?
                    .to_string(),
                media: None,
                quality: AudioQuality::Mp3_128,
                replaygain_db: None,
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
    events: &mut tokio::sync::broadcast::Receiver<hmp_core::PlayerEvent>,
    timeout: Duration,
) {
    tokio::time::timeout(timeout, async {
        loop {
            match events.recv().await {
                Ok(hmp_core::PlayerEvent::PlaybackEnded { .. }) => return,
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
/// 需要真实默认音频设备；无设备 CI 由 hmp-player 的无设备单测覆盖。
/// 固定音质策略（`hmp quality flac`）：回退链从 FLAC 起，不再尝试 Master/HiRes/Atmos。
/// 断言：GetEVkey 序列只含 F0M0（Q0M0/AIM0 不出现）→ 加密失败后明文 M500 兜底。
#[tokio::test]
async fn resolve_track_respects_fixed_quality_config() {
    let _lock = CONFIG_ENV_LOCK.lock().await;
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

    // M500 明文走播放缓存命中（域名固定无法指向 wiremock，见模块注释）。
    seed_playback_cache(
        &format!(
            "https://isure.stream.qqmusic.qq.com/M500{TRACK_MEDIA_MID}.mp3?guid=abc&vkey=testvkey"
        ),
        "",
        b"ID3\x00\x00\x00\x00hmp-e2e-seed",
    );
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

/// 2026-10-02「远程曲目无法播放」根因回归：凭证过期（服务端拒签，每档
/// `result=104003`，2026-10-02 实机复现）时必须上报 `NotLoggedIn`（可操作：
/// 重新登录）而非 `QualityUnavailable`——后者正是用户看到的「音质不存在」
/// 误导文案。回退链本身照常逐档尝试（6 档全试后仍全鉴权失败才归鉴权）。
#[tokio::test]
async fn resolve_track_reports_not_logged_in_when_all_qualities_require_reauth() {
    let _lock = CONFIG_ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::install(dir.path());
    let store: Store = store_from_env();
    store
        .save(&Credential {
            uin: "10001".into(),
            music_id: "10001".into(),
            music_key: "expired-key".into(),
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
    // 加密取流（GetEVkey）：Master/HiRes(AIM0)/Atmos(Q0M0)/Flac(F0M0) 全 104003。
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
            .mount(&server)
            .await;
    }
    // 明文取流（GetVkey）：320/128 也全 104003（过期凭证连免 VIP 档都拒签）。
    for prefix in ["M800", "M500"] {
        let prefix = prefix.to_owned();
        Mock::given(method("POST"))
            .and(path("/cgi-bin/musicu.fcg"))
            .and(move |req: &wiremock::Request| {
                let body = req0(req);
                let filename = body["param"]["filename"][0].as_str().unwrap_or("");
                filename.starts_with(&prefix)
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(urls_fail()))
            .mount(&server)
            .await;
    }

    let resolver = QqSourceResolver::new(client_for(&server.uri()), store);
    let err = resolver
        .resolve_track(&TrackId::new(TRACK_MID))
        .await
        .expect_err("全链 104003 应返回错误");
    assert!(
        matches!(err, EngineError::NotLoggedIn),
        "全链鉴权失败应上报 NotLoggedIn，实际 {err:?}"
    );
    // 回退链确实逐档尝试过（6 档全发请求，而非第一档即放弃）。
    let requested: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| {
            let body = req0(r);
            let module = body["module"].as_str()?.to_owned();
            let f = body["param"]["filename"][0].as_str()?.to_owned();
            Some((module, f))
        })
        .filter(|(m, _)| m == "music.vkey.GetEVkey" || m == "music.vkey.GetVkey")
        .map(|(_, f)| f[..4].to_owned())
        .collect();
    assert_eq!(
        requested,
        vec!["AIM0", "AIM0", "Q0M0", "F0M0", "M800", "M500"],
        "回退链应逐档尝试全部 6 档（Master/HiRes 同映射 AIM0 各一次），实际: {requested:?}"
    );
}

/// 逐档错误聚合回归：混合鉴权（104003）与非鉴权（result=1）失败时上报
/// `QualityUnavailable`，且消息逐档带档位标签——此前只留最后一档错误、
/// 且取流响应缺 midurlinfo 时消息为空串（诊断黑洞）。
#[tokio::test]
async fn resolve_track_aggregates_per_quality_errors_in_message() {
    let _lock = CONFIG_ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::install(dir.path());
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
    // 加密档全部鉴权失败；明文档（320/128）业务码 1（非鉴权，如版权区域限制）。
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
            .mount(&server)
            .await;
    }
    for prefix in ["M800", "M500"] {
        let prefix = prefix.to_owned();
        Mock::given(method("POST"))
            .and(path("/cgi-bin/musicu.fcg"))
            .and(move |req: &wiremock::Request| {
                let body = req0(req);
                let filename = body["param"]["filename"][0].as_str().unwrap_or("");
                filename.starts_with(&prefix)
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(urls_fail_with(1)))
            .mount(&server)
            .await;
    }

    let resolver = QqSourceResolver::new(client_for(&server.uri()), store);
    let err = resolver
        .resolve_track(&TrackId::new(TRACK_MID))
        .await
        .expect_err("全档失败应返回错误");
    match err {
        EngineError::QualityUnavailable(msg) => {
            // 存在非鉴权失败 → 不得误报 NotLoggedIn。
            assert!(
                msg.contains("master: result=104003"),
                "消息应含 master 档鉴权失败，实际: {msg}"
            );
            assert!(
                msg.contains("320: result=1"),
                "消息应含 320 档业务码失败，实际: {msg}"
            );
            assert!(
                msg.contains("128: result=1"),
                "消息应含 128 档业务码失败，实际: {msg}"
            );
        }
        other => panic!("混合失败应报 QualityUnavailable，实际 {other:?}"),
    }
}

/// 流式 `PreparedMedia` 契约（daemon 侧）：`prepare_media` 对支持 Range 的
/// CDN 返回**原 CDN url**（仅元数据）+ 进程内随机访问源（`source` 直供
/// 播放器，取代历史上的 `127.0.0.1` 回环代理）。daemon 拼接的 CDN 域名
/// 固定无法指向 wiremock，故此处直接以 wiremock 充当 Range CDN 验证契约
/// （明文 = IdentityCipher 直通）；resolve_track_impl 对它的调用由测试 1/2
/// 的缓存命中路径与真机测试覆盖。
#[tokio::test]
async fn prepared_media_contract_is_cdn_url_plus_stream_source() {
    let _lock = CONFIG_ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::install(dir.path());

    // 明文音源：尾部 4 字节 [204,205,206,207] 的 LE u32 远超 QMC2 V1 key
    // 上限 0x400 → detect_footer 判 None（不会误入内嵌 ekey 提取）。
    let plaintext: Vec<u8> = {
        let mut v = b"fLaC".to_vec();
        v.extend((0..2000).map(|i| (i % 256) as u8));
        v.extend([204, 205, 206, 207]);
        v
    };
    let total_len = plaintext.len() as u64;

    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", total_len.to_string())
                .insert_header("Accept-Ranges", "bytes"),
        )
        .mount(&server)
        .await;
    let body = plaintext.clone();
    Mock::given(method("GET"))
        .and(header_exists("Range"))
        .respond_with(move |req: &wiremock::Request| match range_of(req) {
            Some((start, end)) if start < total_len => {
                let end_capped = end.min(total_len - 1);
                ResponseTemplate::new(206)
                    .insert_header(
                        "Content-Range",
                        format!("bytes {start}-{end_capped}/{total_len}"),
                    )
                    .set_body_bytes(body[start as usize..=end_capped as usize].to_vec())
            }
            _ => ResponseTemplate::new(416),
        })
        .mount(&server)
        .await;

    let url = format!("{}/song.flac", server.uri());
    let prepared = hmp_media::prepare_media(&url, None, None)
        .await
        .expect("流式 prepare 应成功");
    assert_eq!(prepared.uri, url, "流式路径 uri 保留原 CDN url（元数据）");
    let source = prepared
        .source
        .as_ref()
        .expect("流式路径应携带进程内随机访问源");
    assert_eq!(source.len(), total_len, "明文直通：len = 文件总长");
}

/// 解析请求的 `Range: bytes=start-end` 头。
fn range_of(req: &wiremock::Request) -> Option<(u64, u64)> {
    let v = req.headers.get("Range")?.to_str().ok()?;
    let spec = v.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

#[tokio::test]
#[ignore = "需要真实默认音频设备（真机验收项）"]
async fn play_then_end_advances_queue_with_rodio() {
    // 1) 本地 1s wav（两首，验证续播）
    let dir = tempfile::tempdir().unwrap();
    let wav1 = dir.path().join("t1.wav");
    let wav2 = dir.path().join("t2.wav");
    write_wav(&wav1);
    write_wav(&wav2);

    // 2) 真实 Rodio/CPAL 默认输出驱动
    let driver: Arc<dyn PlaybackDriver> =
        Arc::new(RodioDriver::new().expect("默认音频输出初始化失败"));

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
        st.queue.len == 2
            && st.queue.current == Some(0)
            && st.playback.status == PlaybackStatus::Playing
    })
    .await;
    assert_eq!(handle.queue_rx.borrow().tracks[0], TrackId::new(WAV_ID_1));
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
    //    音频驱动停止后收尾状态接受 Ended | Stopped。
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
