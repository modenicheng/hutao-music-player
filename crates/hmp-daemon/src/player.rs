//! 播放驱动抽象、曲目解析与解析错误（spec §4.2 `player.rs`）。
//!
//! [`PlaybackDriver`] 是后端与播放器的唯一接缝：测试注入 fake，生产用
//! [`GstDriver`]（包 `PlayerCore`）。[`SourceResolver`] 是后端与 QQ API
//! 的唯一接缝：测试注入 fake，生产用 [`QqSourceResolver`]。队列裁决/
//! 自动续播在引擎（`engine.rs`），播放器核心不感知队列。

use std::future::Future;
use std::pin::Pin;

use hmp_core::{
    AlbumId, AlbumRef, ArtistId, ArtistRef, AudioQuality, CoverRef, PlaybackState, PlayerCommand,
    Track, TrackId,
};
use hmp_player_gst::{LoadRequest, PlayerCore, PlayerEvent};
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

/// GStreamer 播放驱动（生产）。
pub struct GstDriver {
    core: PlayerCore,
}

impl std::fmt::Debug for GstDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `PlayerCore` 不实现 Debug；只呈现类型名。
        f.debug_struct("GstDriver").finish_non_exhaustive()
    }
}
impl GstDriver {
    /// 新建（`audio_sink` 为 None 时用系统默认；测试可传 "fakesink"）。
    pub fn new(audio_sink: Option<&str>) -> Result<Self, hmp_core::HmpError> {
        Ok(Self {
            core: PlayerCore::new_with_sink(audio_sink)?,
        })
    }
}

impl PlaybackDriver for GstDriver {
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

/// 解析完成的曲目（含解密 guard，随 daemon 存活）。
pub struct ResolvedTrack {
    /// 领域曲目元数据。
    pub track: Track,
    /// 播放 URI（http://127.0.0.1 代理或 CDN 直连）。
    pub uri: String,
    /// 解密代理 guard（明文播放期间必须持有；换曲时被引擎替换 Drop）。
    pub media: Option<hmp_media::PreparedMedia>,
    /// 本次实际选定的音质（媒体库重构 B3：actual vs available 分离）。
    pub quality: AudioQuality,
}

impl std::fmt::Debug for ResolvedTrack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `PreparedMedia` 不实现 Debug；只呈现是否持有 guard。
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
    #[error("未登录或凭证已过期")]
    NotLoggedIn,
    #[error("曲目不存在")]
    TrackNotFound,
    #[error("歌单/专辑拉取失败: {0}")]
    PlaylistNotFound(String),
    #[error("所有音质均不可用: {0}")]
    QualityUnavailable(String),
    #[error("驱动未在 5s 内应用装载")]
    Timeout,
    #[error("内部错误: {0}")]
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

    /// 直接按 URI 解析（MPRIS `OpenUri`；默认不支持，本地解析器实现 `file://`）。
    fn resolve_uri(
        &self,
        uri: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
        let msg = uri.to_string();
        Box::pin(async move { Err(EngineError::Internal(format!("URI 播放暂不支持: {msg}"))) })
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
            .map_err(|e| EngineError::Internal(format!("读取凭证失败: {e}")))?
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
            resolve_track_impl(&self.client, &credential, &track_id).await
        })
    }
}

/// 音质 → 文件类型（与 CLI play.rs 一致，复制）。
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
pub async fn resolve_track_impl(
    client: &QqMusicClient,
    credential: &hmp_storage::credential::Credential,
    track_id: &TrackId,
) -> Result<ResolvedTrack, EngineError> {
    let song_api = SongApi::new(client);
    let detail = song_api
        .get_detail(track_id.as_ref())
        .await
        .map_err(|e| EngineError::Internal(format!("详情请求失败: {e}")))?;
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
    let chain = hmp_storage::Config::load().quality.chain();
    let file_info = SongFileInfo {
        mid: track_id.as_ref().to_owned(),
        file_type: None,
        song_type: 0,
        media_mid: Some(media_mid),
    };
    let mut last_error = None;
    for quality in chain {
        let Some(file_type) = quality_to_file_type(&quality) else {
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
                        if file_type.is_encrypted {
                            match hmp_media::prepare_stream(
                                &remote_uri,
                                (!item.ekey.is_empty()).then_some(item.ekey.as_str()),
                                None,
                            )
                            .await
                            {
                                Ok(p) => {
                                    let uri = p.uri.clone();
                                    found = Some((file_type, uri, Some(p)));
                                    break;
                                }
                                Err(e) => {
                                    last_error = Some(format!("QMC2 decrypt failed: {e}"));
                                    continue;
                                }
                            }
                        } else {
                            // 明文无需解密 guard：直接播放 CDN URL（media: None）
                            found = Some((file_type, remote_uri, None));
                            break;
                        }
                    } else {
                        last_error = Some(format!("result={}", item.result));
                    }
                }
            }
            Err(e) => {
                last_error = Some(e.to_string());
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
            });
        }
    }
    Err(EngineError::QualityUnavailable(
        last_error.unwrap_or_default(),
    ))
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
            "QQ 解析器不支持本地源（组合解析器负责分发）".into(),
        )),
        hmp_core::PlayRequest::Playlist(id) => {
            let list_id: i64 = id
                .as_ref()
                .parse()
                .map_err(|_| EngineError::PlaylistNotFound("歌单 id 非数字".into()))?;
            let api = SonglistApi::new(client);
            let out = collect_paged(|page| {
                let api = &api;
                async move {
                    let resp = api
                        .get_detail(list_id, 0, 100, page, true, false, false)
                        .await
                        .map_err(|e| EngineError::PlaylistNotFound(e.to_string()))?;
                    let stubs = resp.songs.iter().filter_map(song_stub).collect();
                    Ok((stubs, resp.hasmore != 0, resp.total))
                }
            })
            .await?;
            if out.is_empty() {
                return Err(EngineError::PlaylistNotFound("歌单为空".into()));
            }
            Ok(out)
        }
        hmp_core::PlayRequest::Album(id) => {
            let api = AlbumApi::new(client);
            let out = collect_paged(|page| {
                let api = &api;
                async move {
                    let resp = api
                        .get_song(id.as_ref(), 100, page)
                        .await
                        .map_err(|e| EngineError::PlaylistNotFound(e.to_string()))?;
                    let stubs = resp.song_list.iter().filter_map(song_stub).collect();
                    Ok((stubs, true, resp.total_num))
                }
            })
            .await?;
            if out.is_empty() {
                return Err(EngineError::PlaylistNotFound("专辑为空".into()));
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
pub fn available_from_sizes(f: &hmp_qqmusic_api::models::File) -> Vec<AudioQuality> {
    let mut available = Vec::new();
    if f.size_dolby > 0 {
        available.push(AudioQuality::Atmos);
    }
    if f.size_flac > 0 {
        available.push(AudioQuality::Flac);
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
            size_flac: 1,
            size_dolby: 1,
            ..Default::default()
        };
        assert_eq!(
            available_from_sizes(&all),
            vec![
                AudioQuality::Atmos,
                AudioQuality::Flac,
                AudioQuality::Mp3_320,
                AudioQuality::Mp3_128
            ]
        );
        let none = hmp_qqmusic_api::models::File::default();
        assert!(available_from_sizes(&none).is_empty());
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
