//! 播放驱动抽象、曲目解析与解析错误（spec §4.2 `player.rs`）。
//!
//! [`PlaybackDriver`] 是后端与播放器的唯一接缝：测试注入 fake，生产用
//! [`RodioDriver`]（包 `PlayerCore`）。[`SourceResolver`] 是后端与 QQ API
//! 的唯一接缝：测试注入 fake，生产用 [`QqSourceResolver`]。队列裁决/
//! 自动续播在引擎（`engine.rs`），播放器核心不感知队列。

use std::future::Future;
use std::pin::Pin;

use hmp_core::{
    AlbumId, AlbumRef, ArtistId, ArtistRef, AudioQuality, CoverRef, LoadRequest, PlaybackState,
    PlayerCommand, PlayerEvent, Track, TrackId,
};
use hmp_player::PlayerCore;
use hmp_qqmusic_api::pagination::Page;
use hmp_qqmusic_api::{AlbumApi, QqMusicClient, SongApi, SongFileInfo, SongFileType, SonglistApi};
use hmp_storage::credential::Store;
use tokio::sync::{broadcast, watch};

/// 播放驱动（同步接缝）。
pub trait PlaybackDriver: Send + Sync {
    /// 加载曲目（URI 已就绪）。
    fn load(&self, request: LoadRequest);
    fn play(&self);
    fn pause(&self);
    fn seek(&self, position: std::time::Duration);
    fn stop(&self);
    fn set_volume(&self, volume: f64);
    /// 转发通用命令（Play/Pause/Stop/Seek/Volume/Loop/Shuffle/TogglePlay）。
    /// Next/Previous/LoadAndPlay 由引擎拦截，不转发。
    fn command(&self, cmd: PlayerCommand);
    fn shutdown(&self);
    /// 播放状态（watch 单一来源）。
    fn subscribe_state(&self) -> watch::Receiver<PlaybackState>;
    /// 播放器离散事件（Ended/Error）。
    fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent>;
}

/// Rodio/CPAL 播放驱动（生产）。
pub struct RodioDriver {
    core: PlayerCore,
}

impl std::fmt::Debug for RodioDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `PlayerCore` 不实现 Debug；只呈现类型名。
        f.debug_struct("RodioDriver").finish_non_exhaustive()
    }
}
impl RodioDriver {
    /// 使用平台默认音频输出新建驱动。
    pub fn new() -> Result<Self, hmp_core::HmpError> {
        Ok(Self {
            core: PlayerCore::new()?,
        })
    }
}

impl PlaybackDriver for RodioDriver {
    fn load(&self, request: LoadRequest) {
        self.core.load(request);
    }
    fn play(&self) {
        self.core.play();
    }
    fn pause(&self) {
        self.core.pause();
    }
    fn seek(&self, position: std::time::Duration) {
        self.core.seek(position);
    }
    fn stop(&self) {
        self.core.stop();
    }
    fn set_volume(&self, volume: f64) {
        self.core.set_volume(volume);
    }
    fn command(&self, cmd: PlayerCommand) {
        let _ = self.core.command_sender().send(cmd);
    }
    fn shutdown(&self) {
        self.core.shutdown();
    }
    fn subscribe_state(&self) -> watch::Receiver<PlaybackState> {
        self.core.subscribe_state()
    }
    fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent> {
        self.core.subscribe_events()
    }
}

/// 解析完成的曲目（远端流式路径含进程内解密源）。
pub struct ResolvedTrack {
    /// 领域曲目元数据。
    pub track: Track,
    /// 播放 URI（远端流式 = 原 CDN url，仅元数据/日志；CDN 无 Range
    /// 回退与播放缓存命中 = `file://`）。
    pub uri: String,
    /// 进程内解密源（流式路径 `Some`，字节流经 `source` 直供播放器）；
    /// 缓存命中/回退（`file://`）与本地曲目为 `None`。生命周期随
    /// reader/引擎 `AppliedLoad.source`（reader 存活即源存活，无需
    /// 引擎单独保活）。
    pub media: Option<hmp_media::PreparedMedia>,
    /// 本次实际选定的音质（媒体库重构 B3：actual vs available 分离）。
    pub quality: AudioQuality,
    /// ReplayGain 曲目增益（dB；无标签 None → 不补偿）。
    pub replaygain_db: Option<f64>,
}

impl std::fmt::Debug for ResolvedTrack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `PreparedMedia` 不实现 Debug；只呈现是否持有进程内源。
        f.debug_struct("ResolvedTrack")
            .field("track", &self.track)
            .field("uri", &self.uri)
            .field("media", &self.media.is_some())
            .finish()
    }
}

/// 解析错误（引擎内部；映射为 `IpcErrorCode`）。
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("not logged in or credentials expired")]
    NotLoggedIn,
    #[error("track not found")]
    TrackNotFound,
    #[error("failed to fetch playlist/album: {0}")]
    PlaylistNotFound(String),
    #[error("no available audio quality: {0}")]
    QualityUnavailable(String),
    /// 驱动装载失败（打开/解码错误，驱动 Error 事件即时返回）。引擎据此
    /// 触发音质降档重试（AUDIT §16 开放项：解码期失败纳入回退链）。
    #[error("driver load failed: {0}")]
    LoadFailed(String),
    #[error("driver did not apply the load before the load timeout")]
    Timeout,
    #[error("internal error: {0}")]
    Internal(String),
}

/// 播放源解析接缝（引擎唯一网络入口）。
///
/// 返回 `BoxFuture`（而非 RPITIT）：RPITIT 的 `impl Future + Send` 返回类型
/// 会使 trait 失去 dyn 兼容性（E0038），而引擎以 `Arc<dyn SourceResolver>`
/// 持有本接缝（见计划 Task 2 Step 3 的备选说明）。
pub trait SourceResolver: Send + Sync + std::fmt::Debug {
    /// 解析源为曲目列表（单曲=1 个；歌单/专辑=分页拉取）。
    /// 返回 [`hmp_core::TrackStub`]：列表解析已带出标题/歌手/时长，
    /// 由引擎批量缓存进媒体库（投影层查询用），不再丢弃为纯 ID。
    fn resolve_source_ids(
        &self,
        src: &hmp_core::PlayRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>>;

    /// 解析单曲为可播放 URI + 元数据（音质回退 + QMC2 解密）。
    fn resolve_track(
        &self,
        track_id: &TrackId,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>>;

    /// 同 [`SourceResolver::resolve_track`]，但排除指定音质档位
    /// （装载期解码失败降档重试：失败档不得再次解析出来，否则死循环）。
    /// 默认忽略排除（fake/本地解析器无音质链概念）；生产解析器覆写。
    fn resolve_track_excluding(
        &self,
        track_id: &TrackId,
        exclude: &[AudioQuality],
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
        let _ = exclude;
        self.resolve_track(track_id)
    }

    /// 直接按 URI 解析（MPRIS `OpenUri`；默认不支持，本地解析器实现 `file://`）。
    fn resolve_uri(
        &self,
        uri: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
        let msg = uri.to_string();
        Box::pin(async move {
            Err(EngineError::Internal(format!(
                "URI playback not supported: {msg}"
            )))
        })
    }
}

/// 生产解析器（QQ API + 共享凭证）。
pub struct QqSourceResolver {
    client: QqMusicClient,
    store: Store,
}

impl std::fmt::Debug for QqSourceResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `QqMusicClient` 与 `Store` 均不实现 Debug；只呈现凭证状态，
        // 避免在日志中暴露敏感字段。
        f.debug_struct("QqSourceResolver")
            .field("credential", &self.has_credential())
            .finish()
    }
}

impl QqSourceResolver {
    /// 新建（`store` 由 `store_from_env()` 构造）。
    pub fn new(client: QqMusicClient, store: Store) -> Self {
        Self { client, store }
    }

    /// 当前是否有有效凭证（供服务器同步前置校验）。
    pub fn has_credential(&self) -> bool {
        self.store
            .load()
            .ok()
            .flatten()
            .is_some_and(|c| c.is_logged_in())
    }

    fn load_credential(&self) -> Result<hmp_storage::credential::Credential, EngineError> {
        self.store
            .load()
            .map_err(|e| EngineError::Internal(format!("failed to read credentials: {e}")))?
            .ok_or(EngineError::NotLoggedIn)
    }
}

impl SourceResolver for QqSourceResolver {
    fn resolve_source_ids(
        &self,
        src: &hmp_core::PlayRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>>
    {
        // 克隆 src：让 future 持有数据，不借用参数（返回类型生命周期为 `&self`）。
        let src = src.clone();
        Box::pin(async move {
            self.load_credential()?;
            resolve_source_ids_impl(&self.client, &src).await
        })
    }

    fn resolve_track(
        &self,
        track_id: &TrackId,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
        // 克隆 id：让 future 持有数据，不借用参数（返回类型生命周期为 `&self`）。
        let track_id = track_id.clone();
        Box::pin(async move {
            let credential = self.load_credential()?;
            resolve_track_impl(&self.client, &credential, &track_id, &[]).await
        })
    }

    fn resolve_track_excluding(
        &self,
        track_id: &TrackId,
        exclude: &[AudioQuality],
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
        let track_id = track_id.clone();
        let exclude = exclude.to_vec();
        Box::pin(async move {
            let credential = self.load_credential()?;
            resolve_track_impl(&self.client, &credential, &track_id, &exclude).await
        })
    }
}

/// 音质 → 文件类型（QQ 文件类型档案的唯一映射点）。
///
/// `HiRes` 映射到 `SongFileType::MASTER`（AIM0）是**有意**的：上游无独立
/// Hi-Res 文件类型，MASTER 即「臻品母带 = FLAC 24Bit/192kHz」档（qqmusic-api
/// 文档），也就是 Hi-Res 产品本身。QQ 侧不存在 F1M0 等独立 Hi-Res 档位，
/// 因此 `hmp quality hires` 与 `hmp quality master` 请求同一档（回退链不同）。
fn quality_to_file_type(q: &AudioQuality) -> Option<SongFileType> {
    use AudioQuality::*;
    match q {
        Master => Some(SongFileType::MASTER),
        HiRes => Some(SongFileType::MASTER),
        Atmos => Some(SongFileType::ATMOS_2),
        Flac => Some(SongFileType::FLAC),
        Aac => Some(SongFileType::AAC_192),
        Mp3_320 => Some(SongFileType::MP3_320),
        Mp3_128 => Some(SongFileType::MP3_128),
        Unknown(_) => None,
    }
}

/// 解析单个曲目 → 可播放 URI + 元数据（音质回退 + QMC2 解密）。
///
/// `exclude`：跳过的音质档位（装载期解码失败降档重试时由引擎传入失败档，
/// 防止同一坏档被再次解析出来）。
pub async fn resolve_track_impl(
    client: &QqMusicClient,
    credential: &hmp_storage::credential::Credential,
    track_id: &TrackId,
    exclude: &[AudioQuality],
) -> Result<ResolvedTrack, EngineError> {
    let song_api = SongApi::new(client);
    let detail = song_api
        .get_detail(track_id.as_ref())
        .await
        .map_err(|e| EngineError::Internal(format!("track detail request failed: {e}")))?;
    let media_mid = detail.track.file.media_mid.clone();
    if media_mid.is_empty() {
        return Err(EngineError::TrackNotFound);
    }
    // 元数据（歌手/专辑/封面，供 MPRIS）
    let singers = detail
        .track
        .singer
        .iter()
        .filter(|s| !s.name.is_empty())
        .map(|s| ArtistRef {
            id: ArtistId::new(if s.mid.is_empty() {
                s.id.to_string()
            } else {
                s.mid.clone()
            }),
            name: s.name.clone(),
        })
        .collect::<Vec<_>>();
    let album = (!detail.track.album.name.is_empty()).then(|| AlbumRef {
        id: AlbumId::new(detail.track.album.mid.clone()),
        name: detail.track.album.name.clone(),
    });
    let cover = (!detail.track.album.pmid.is_empty()).then(|| CoverRef {
        url: format!(
            "https://y.gtimg.cn/music/photo_new/T002R300x300M000{}.jpg",
            detail.track.album.pmid
        ),
    });
    let title = detail.track.name.clone();

    // 可用音质初值：QQ size 字段（确定映射的档位），从高到低去重。
    let mut available = available_from_sizes(&detail.track.file);

    // 音质回退链：来自持久化偏好（`hmp quality`；Auto = 文档化链
    // Master→HiRes→Atmos→Flac→Mp3_320→Mp3_128，固定档位则从该档起降级）。
    // 排除失败档（解码期降档重试）后为空 → 无档可试。
    let chain: Vec<AudioQuality> = hmp_storage::Config::load()
        .quality
        .chain()
        .into_iter()
        .filter(|q| !exclude.contains(q))
        .collect();
    if chain.is_empty() {
        return Err(EngineError::QualityUnavailable(format!(
            "every quality already failed loading: {}",
            exclude
                .iter()
                .map(|q| q.to_alias())
                .collect::<Vec<_>>()
                .join("/")
        )));
    }
    let file_info = SongFileInfo {
        mid: track_id.as_ref().to_owned(),
        file_type: None,
        song_type: 0,
        media_mid: Some(media_mid),
    };
    // 逐档错误聚合（此前只留最后一档错误，且取流响应缺 midurlinfo 时
    // 恒为空串——诊断黑洞）；`all_auth` 追踪「是否所有失败档都是鉴权码」：
    // 全链鉴权失败（凭证过期/未登录）时上游表现为每档 result=104003/101404，
    // 应上报 NotLoggedIn（可操作：重新登录）而非误导性的音质不可用。
    let mut rung_errors: Vec<String> = Vec::new();
    let mut all_auth = true;
    for quality in chain {
        let alias = quality.to_alias();
        let Some(file_type) = quality_to_file_type(&quality) else {
            all_auth = false;
            rung_errors.push(format!("{alias}: no file type mapping"));
            continue;
        };
        let urls = song_api
            .get_song_urls(
                std::slice::from_ref(&file_info),
                file_type,
                Some(credential),
            )
            .await;
        let mut found: Option<(SongFileType, String, Option<hmp_media::PreparedMedia>)> = None;
        match urls {
            Ok(resp) => {
                for item in &resp.data {
                    if item.result == 0 && !item.purl.is_empty() {
                        let remote_uri =
                            format!("https://isure.stream.qqmusic.qq.com/{}", item.purl);
                        let ekey_opt = (!item.ekey.is_empty()).then_some(item.ekey.as_str());
                        // 播放缓存命中 → 直接播本地（零 CDN；键=URL path|ekey
                        // 跨 purl 重签稳定，与回退/回填路径同键空间）。
                        match hmp_media::cached_playable_uri(&remote_uri, ekey_opt) {
                            Ok(Some(local_uri)) => {
                                found = Some((file_type, local_uri, None));
                                break;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tracing::debug!(%e, "playback cache lookup failed; streaming");
                            }
                        }
                        // 加密/明文统一经 prepare_media：加密走 QMC2 流密码，
                        // 明文（ekey 空且无内嵌 footer）走 IdentityCipher 直通。
                        // CDN 支持 Range → 进程内随机访问源（uri 保留原 CDN url，
                        // 播放字节流经 source 直供播放器）；无 Range → hmp-media
                        // 自动回退全量下载-解密-缓存（uri = file://，source None）。
                        // 流式 reader 自带 tee 边播边缓存（顺序消费写入缓存，
                        // drop 后后台补齐剩余区间），二次播放由上方
                        // cached_playable_uri 命中——解析层不再后台二次全量下载
                        // （未播放的预解析曲目不回填缓存）。
                        match hmp_media::prepare_media(&remote_uri, ekey_opt, None).await {
                            Ok(p) => {
                                let uri = p.uri.clone();
                                found = Some((file_type, uri, Some(p)));
                                break;
                            }
                            Err(e) => {
                                all_auth = false;
                                rung_errors.push(format!("{alias}: prepare media failed: {e}"));
                                continue;
                            }
                        }
                    } else {
                        all_auth &= is_auth_result_code(item.result);
                        rung_errors.push(format!("{alias}: result={}", item.result));
                    }
                }
                if resp.data.is_empty() {
                    // 服务端应答缺 midurlinfo（异常形态）：归因不明，
                    // 保守不视为鉴权失败。
                    all_auth = false;
                    rung_errors.push(format!("{alias}: empty midurlinfo"));
                }
            }
            Err(e) => {
                all_auth &= is_auth_transport_error(&e);
                rung_errors.push(format!("{alias}: {e}"));
            }
        }
        if let Some((file_type, uri, media)) = found {
            // 成功档位并入可用列表（探测结果）。
            let q = quality_from_file_type(&file_type);
            if !available.contains(&q) {
                available.push(q.clone());
            }
            available.sort_by_key(|q| {
                AudioQuality::ordered()
                    .iter()
                    .position(|x| x == q)
                    .unwrap_or(usize::MAX)
            });
            let track = Track {
                id: track_id.clone(),
                title,
                artists: singers,
                album,
                duration: detail
                    .track
                    .interval
                    .checked_mul(1000)
                    .and_then(|ms| u64::try_from(ms).ok())
                    .map(std::time::Duration::from_millis),
                cover,
                url: Some(uri.clone()),
                available_qualities: available,
            };
            return Ok(ResolvedTrack {
                track,
                uri,
                media,
                quality: q,
                replaygain_db: None, // QQ 曲目无 RG 标签源
            });
        }
    }
    debug_assert!(
        !rung_errors.is_empty(),
        "chain non-empty and nothing succeeded → 每档必有错误记录"
    );
    if all_auth {
        // 全链鉴权失败：凭证缺失/过期/无权限（未登录时 load_credential 已拦，
        // 这里是「有凭证但服务端判无效」的实况）。上报 NotLoggedIn 而非
        // QualityUnavailable（2026-10-02 实机：过期凭证全链 104003，用户看到
        // 「音质不存在」实为登录态失效）。
        return Err(EngineError::NotLoggedIn);
    }
    Err(EngineError::QualityUnavailable(rung_errors.join("; ")))
}

/// 取流单项业务码是否为鉴权类失败（101404=需登录，104003=无权限/需登录态；
/// 见 `UrlinfoItem::result` 文档与 2026-08-06/09-29 取流实测记录）。
fn is_auth_result_code(result: i64) -> bool {
    result == 101404 || result == 104003
}

/// 取流请求级错误是否为鉴权类失败（凭证缺失/过期变体；业务码形态见
/// [`is_auth_result_code`]）。
fn is_auth_transport_error(e: &hmp_qqmusic_api::QqMusicError) -> bool {
    matches!(
        e,
        hmp_qqmusic_api::QqMusicError::AuthenticationRequired
            | hmp_qqmusic_api::QqMusicError::CredentialExpired
            | hmp_qqmusic_api::QqMusicError::LoginAuthExpired
    )
}

/// 解析源为 TrackId 列表（单曲/歌单/专辑；歌单/专辑分页拉取，
/// 以服务端 hasmore/total 为终止条件，安全上限 `MAX_PAGES` 页防死循环）。
pub async fn resolve_source_ids_impl(
    client: &QqMusicClient,
    src: &hmp_core::PlayRequest,
) -> Result<Vec<hmp_core::TrackStub>, EngineError> {
    match src {
        hmp_core::PlayRequest::Track(id) => Ok(vec![id_stub(id)]),
        hmp_core::PlayRequest::Local(_) => Err(EngineError::Internal(
            "QQ resolver does not support local sources (handled by the combined resolver)".into(),
        )),
        hmp_core::PlayRequest::LibraryPlaylist(_) => Err(EngineError::Internal(
            "QQ resolver does not support local playlists (handled by the combined resolver)"
                .into(),
        )),
        hmp_core::PlayRequest::Playlist(id) => {
            let list_id: i64 = id
                .as_ref()
                .parse()
                .map_err(|_| EngineError::PlaylistNotFound("playlist id is not numeric".into()))?;
            let api = SonglistApi::new(client);
            let out = collect_paged(|page| {
                let api = &api;
                async move {
                    let resp = api
                        .get_detail(list_id, 0, Page::new(page as u32, 100), true, false, false)
                        .await
                        .map_err(|e| EngineError::PlaylistNotFound(e.to_string()))?;
                    let stubs = resp.songs.iter().filter_map(song_stub).collect();
                    Ok((stubs, resp.hasmore != 0, resp.total))
                }
            })
            .await?;
            if out.is_empty() {
                return Err(EngineError::PlaylistNotFound("playlist is empty".into()));
            }
            Ok(out)
        }
        hmp_core::PlayRequest::Album(id) => {
            let api = AlbumApi::new(client);
            let out = collect_paged(|page| {
                let api = &api;
                async move {
                    let resp = api
                        .get_song(id.as_ref(), Page::new(page as u32, 100))
                        .await
                        .map_err(|e| EngineError::PlaylistNotFound(e.to_string()))?;
                    let stubs = resp.song_list.iter().filter_map(song_stub).collect();
                    Ok((stubs, true, resp.total_num))
                }
            })
            .await?;
            if out.is_empty() {
                return Err(EngineError::PlaylistNotFound("album is empty".into()));
            }
            Ok(out)
        }
    }
}

/// 单曲源：无列表元数据，title 回退为 id（播放/收藏时由详情/投影补充）。
fn id_stub(id: &TrackId) -> hmp_core::TrackStub {
    hmp_core::TrackStub {
        id: id.clone(),
        title: id.to_string(),
        artists: Vec::new(),
        album: None,
        duration_ms: None,
    }
}

/// QQ `Song` → [`hmp_core::TrackStub`]（列表解析附带元数据，供媒体库批量缓存）。
fn song_stub(s: &hmp_qqmusic_api::models::Song) -> Option<hmp_core::TrackStub> {
    if s.mid.is_empty() {
        return None;
    }
    let title = if s.name.is_empty() {
        if s.title.is_empty() {
            s.mid.clone()
        } else {
            s.title.clone()
        }
    } else {
        s.name.clone()
    };
    Some(hmp_core::TrackStub {
        id: hmp_core::TrackId::new(s.mid.clone()),
        title,
        artists: s.singer.iter().map(|x| x.name.clone()).collect(),
        album: (!s.album.name.is_empty()).then(|| s.album.name.clone()),
        duration_ms: (s.interval > 0).then(|| s.interval * 1000),
    })
}

/// 从 QQ size 字段探测可用音质（确定映射的档位，从高到低；媒体库重构 B3）。
///
/// 仅映射有独立 size 字段的档位（线上 JSON 键名 2026-10-02 实机 dump 核对）：
/// `size_hires`→HiRes、`size_dolby`→Atmos、`size_flac`→Flac、
/// `size_192aac`→Aac、`size_320mp3`→Mp3_320、`size_128mp3`→Mp3_128。
/// 臻品母带在 `size_new` 数组内（索引→档位无上游文档，不猜）。
/// 注意 Atmos 的 size 字段是杜比全景声（D0M4）而回退链尝试的是臻品音质
/// 2.0（Q0M0）——家族内近似档；取流失败/解码失败由回退链降档兜底。
pub fn available_from_sizes(f: &hmp_qqmusic_api::models::File) -> Vec<AudioQuality> {
    let mut available = Vec::new();
    if f.size_hires > 0 {
        available.push(AudioQuality::HiRes);
    }
    if f.size_dolby > 0 {
        available.push(AudioQuality::Atmos);
    }
    if f.size_flac > 0 {
        available.push(AudioQuality::Flac);
    }
    if f.size_192aac > 0 {
        available.push(AudioQuality::Aac);
    }
    if f.size_320mp3 > 0 {
        available.push(AudioQuality::Mp3_320);
    }
    if f.size_128mp3 > 0 {
        available.push(AudioQuality::Mp3_128);
    }
    available
}

/// 分页收集安全上限（100 页 × 100 首/页 = 1 万首，防服务端异常死循环）。
pub const MAX_PAGES: i64 = 100;

/// 分页收集：以服务端终止条件收尾，而非固定页数。
/// `fetch(page)` 返回 (stubs, hasmore, total)；hasmore=false、
/// 已收集 ≥ total、或超过 `MAX_PAGES` 页时停止。
pub async fn collect_paged<F, Fut>(mut fetch: F) -> Result<Vec<hmp_core::TrackStub>, EngineError>
where
    F: FnMut(i64) -> Fut,
    Fut: Future<Output = Result<(Vec<hmp_core::TrackStub>, bool, i64), EngineError>>,
{
    let mut out = Vec::new();
    let mut page = 1i64;
    loop {
        let (mids, hasmore, total) = fetch(page).await?;
        out.extend(mids);
        if !hasmore || out.len() as i64 >= total || page >= MAX_PAGES {
            break;
        }
        page += 1;
    }
    Ok(out)
}

/// 反向映射（展示用）。
fn quality_from_file_type(t: &SongFileType) -> AudioQuality {
    match (t.s, t.e) {
        ("AIM0", _) => AudioQuality::Master,
        ("Q0M0", _) => AudioQuality::Atmos,
        ("F0M0", _) => AudioQuality::Flac,
        ("C600", _) => AudioQuality::Aac,
        ("M800", _) => AudioQuality::Mp3_320,
        _ => AudioQuality::Mp3_128,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 显式回退链（final review Finding 3）须覆盖全部 6 档且不含 Aac：
    /// Master → HiRes → Atmos → Flac → Mp3_320 → Mp3_128。
    #[test]
    fn explicit_fallback_chain_has_atmos_no_aac() {
        const CHAIN: [AudioQuality; 6] = [
            AudioQuality::Master,
            AudioQuality::HiRes,
            AudioQuality::Atmos,
            AudioQuality::Flac,
            AudioQuality::Mp3_320,
            AudioQuality::Mp3_128,
        ];
        assert_eq!(CHAIN.len(), 6);
        assert!(CHAIN.contains(&AudioQuality::Atmos));
        assert!(!CHAIN.contains(&AudioQuality::Aac));
        // 与文档化链（docs/PROJECT.md §7.3）一致
        assert_eq!(CHAIN[0], AudioQuality::Master);
        assert_eq!(CHAIN[2], AudioQuality::Atmos);
        assert_eq!(CHAIN[5], AudioQuality::Mp3_128);
    }

    /// size 字段 → 可用音质（从高到低；缺失档位不出现）。
    /// HiRes 有意映射 MASTER（上游无独立 Hi-Res 类型；MASTER = 24Bit/192kHz）。
    #[test]
    fn hires_maps_to_master_file_type() {
        assert_eq!(
            quality_to_file_type(&AudioQuality::HiRes),
            Some(SongFileType::MASTER)
        );
        assert_eq!(
            quality_to_file_type(&AudioQuality::Master),
            Some(SongFileType::MASTER)
        );
        assert_eq!(
            quality_to_file_type(&AudioQuality::Flac),
            Some(SongFileType::FLAC)
        );
        assert_eq!(
            quality_to_file_type(&AudioQuality::Mp3_320),
            Some(SongFileType::MP3_320)
        );
    }

    #[test]
    fn available_from_sizes_maps_definite_qualities() {
        let f = hmp_qqmusic_api::models::File {
            media_mid: "m".into(),
            size_128mp3: 1,
            size_320mp3: 1,
            size_flac: 0,
            size_dolby: 0,
            ..Default::default()
        };
        assert_eq!(
            available_from_sizes(&f),
            vec![AudioQuality::Mp3_320, AudioQuality::Mp3_128]
        );
        let all = hmp_qqmusic_api::models::File {
            media_mid: "m".into(),
            size_128mp3: 1,
            size_320mp3: 1,
            size_192aac: 1,
            size_flac: 1,
            size_hires: 1,
            size_dolby: 1,
            ..Default::default()
        };
        assert_eq!(
            available_from_sizes(&all),
            vec![
                AudioQuality::HiRes,
                AudioQuality::Atmos,
                AudioQuality::Flac,
                AudioQuality::Aac,
                AudioQuality::Mp3_320,
                AudioQuality::Mp3_128
            ]
        );
        let none = hmp_qqmusic_api::models::File::default();
        assert!(available_from_sizes(&none).is_empty());
    }

    /// 音质三表自洽回归（2026-10-02「远程曲目无法播放」排查锚点）：
    /// `available_from_sizes` 覆盖的档位 ↔ `quality_to_file_type` 前缀 ↔
    /// `quality_from_file_type` 反向映射须一致；HiRes 有意共用 MASTER
    /// （上游无独立 Hi-Res 类型），正向映射为 MASTER、反向归 Master。
    #[test]
    fn quality_maps_roundtrip_consistently() {
        // 正向：ordered 全档位均可映射（回退链每档都真实被尝试，不会静默跳档）。
        for q in AudioQuality::ordered() {
            assert!(quality_to_file_type(&q).is_some(), "{q:?} 应有文件类型映射");
        }
        // 反向：映射出的文件类型经 quality_from_file_type 回到原档位
        // （HiRes 例外：与 Master 同档 AIM0，反向归 Master——有意设计）。
        for q in AudioQuality::ordered() {
            let t = quality_to_file_type(&q).unwrap();
            let back = quality_from_file_type(&t);
            let expected = if q == AudioQuality::HiRes {
                AudioQuality::Master
            } else {
                q.clone()
            };
            assert_eq!(back, expected, "{q:?} → {t:?} → {back:?} 反向映射不一致");
        }
        // size 字段（与线上 JSON 键同名）覆盖档位从高到低有序。
        // Master 不在 size 探测范围（臻品系列 size 在 size_new 数组内，
        // 索引→档位无上游文档）；Aac 可探测但不在文档化回退链内。
        let f = hmp_qqmusic_api::models::File {
            size_hires: 1,
            size_dolby: 1,
            size_flac: 1,
            size_192aac: 1,
            size_320mp3: 1,
            size_128mp3: 1,
            ..Default::default()
        };
        assert_eq!(
            available_from_sizes(&f),
            vec![
                AudioQuality::HiRes,
                AudioQuality::Atmos,
                AudioQuality::Flac,
                AudioQuality::Aac,
                AudioQuality::Mp3_320,
                AudioQuality::Mp3_128
            ]
        );
    }

    /// 鉴权类失败分类：取流业务码（101404 需登录 / 104003 无权限）与
    /// 请求级错误变体；其余（网络/HTTP/普通业务码）不得误判为鉴权。
    /// 回归锚点：过期凭证全链 104003 必须上报 NotLoggedIn 而非
    /// QualityUnavailable（2026-10-02 实机复现「音质不存在」误导）。
    #[test]
    fn auth_failure_classification() {
        assert!(is_auth_result_code(104003));
        assert!(is_auth_result_code(101404));
        assert!(!is_auth_result_code(0));
        assert!(!is_auth_result_code(1));
        assert!(!is_auth_result_code(1014040));

        use hmp_qqmusic_api::QqMusicError;
        assert!(is_auth_transport_error(
            &QqMusicError::AuthenticationRequired
        ));
        assert!(is_auth_transport_error(&QqMusicError::CredentialExpired));
        assert!(is_auth_transport_error(&QqMusicError::LoginAuthExpired));
        assert!(!is_auth_transport_error(&QqMusicError::Network(
            "timeout".into()
        )));
        assert!(!is_auth_transport_error(&QqMusicError::Http {
            status: 503,
            message: "unavailable".into()
        }));
        assert!(!is_auth_transport_error(&QqMusicError::QqApi {
            code: 1,
            message: "fail".into()
        }));
    }

    /// 分页：以服务端 hasmore/total 为终止条件，超过 3 页也能取全（旧代码 3×100 截断）。
    #[tokio::test]
    async fn collect_paged_fetches_beyond_three_pages() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let ids = {
            let calls = calls.clone();
            collect_paged(move |page| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let page = page as u32;
                    let start = (page - 1) * 100;
                    let stubs = (start..start + 100)
                        .map(|i| hmp_core::TrackStub {
                            id: TrackId::new(i.to_string()),
                            title: format!("t{i}"),
                            artists: Vec::new(),
                            album: None,
                            duration_ms: None,
                        })
                        .collect();
                    // 4 页共 400 首，前三页 hasmore=1
                    Ok((stubs, page < 4, 400))
                }
            })
            .await
            .unwrap()
        };
        assert_eq!(ids.len(), 400, "应取全部 400 首而非 3 页截断");
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 4);
        assert_eq!(ids[399].id.as_ref(), "399");
    }

    /// 分页：hasmore=false 提前终止，不取多余页。
    #[tokio::test]
    async fn collect_paged_stops_on_hasmore_false() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let ids = {
            let calls = calls.clone();
            collect_paged(move |page| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let page = page as u32;
                    let stubs = vec![hmp_core::TrackStub {
                        id: TrackId::new(format!("p{page}")),
                        title: format!("p{page}"),
                        artists: Vec::new(),
                        album: None,
                        duration_ms: None,
                    }];
                    Ok((stubs, page < 2, 9999)) // total 很大但 hasmore=false 即停
                }
            })
            .await
            .unwrap()
        };
        assert_eq!(ids.len(), 2);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    /// 分页：total 达到即停（服务端总数少时不多拉）。
    #[tokio::test]
    async fn collect_paged_stops_at_total() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let ids = {
            let calls = calls.clone();
            collect_paged(move |page| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let page = page as u32;
                    let stubs = (0..50)
                        .map(|i| hmp_core::TrackStub {
                            id: TrackId::new(format!("p{page}-{i}")),
                            title: format!("p{page}-{i}"),
                            artists: Vec::new(),
                            album: None,
                            duration_ms: None,
                        })
                        .collect();
                    Ok((stubs, true, 150)) // 3 页 × 50 = 150
                }
            })
            .await
            .unwrap()
        };
        assert_eq!(ids.len(), 150);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 3);
    }

    /// QQ `Song` → stub：列表解析附带元数据，标题回退 mid。
    #[test]
    fn song_stub_extracts_metadata() {
        use hmp_qqmusic_api::models::{Album, Singer, Song};
        let s = Song {
            mid: "003OUlho2HcRHC".into(),
            name: "夜曲".into(),
            singer: vec![Singer {
                name: "周杰伦".into(),
                ..Default::default()
            }],
            album: Album {
                name: "十一月的萧邦".into(),
                ..Default::default()
            },
            interval: 193,
            ..Default::default()
        };
        let stub = song_stub(&s).unwrap();
        assert_eq!(stub.id.as_ref(), "003OUlho2HcRHC");
        assert_eq!(stub.title, "夜曲");
        assert_eq!(stub.artists, vec!["周杰伦"]);
        assert_eq!(stub.album.as_deref(), Some("十一月的萧邦"));
        assert_eq!(stub.duration_ms, Some(193_000));
        // 空 mid 丢弃；缺元数据时 title 回退 mid。
        assert!(song_stub(&Song::default()).is_none());
        let bare = song_stub(&Song {
            mid: "mid-x".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(bare.title, "mid-x");
        assert!(bare.artists.is_empty());
        assert_eq!(bare.duration_ms, None);
    }

    /// 音质 → 文件类型映射：Atmos 必须可映射（链中尝试时不会因 None 跳过）。
    #[test]
    fn quality_to_file_type_maps_atmos_and_aac() {
        assert_eq!(
            quality_to_file_type(&AudioQuality::Atmos),
            Some(SongFileType::ATMOS_2)
        );
        assert_eq!(
            quality_to_file_type(&AudioQuality::Aac),
            Some(SongFileType::AAC_192)
        );
        assert_eq!(
            quality_to_file_type(&AudioQuality::Master),
            Some(SongFileType::MASTER)
        );
        assert_eq!(
            quality_to_file_type(&AudioQuality::Flac),
            Some(SongFileType::FLAC)
        );
        assert_eq!(
            quality_to_file_type(&AudioQuality::Mp3_320),
            Some(SongFileType::MP3_320)
        );
        assert_eq!(
            quality_to_file_type(&AudioQuality::Mp3_128),
            Some(SongFileType::MP3_128)
        );
        assert_eq!(
            quality_to_file_type(&AudioQuality::Unknown("X".into())),
            None
        );
    }
}
