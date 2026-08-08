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
    #[error("内部错误: {0}")]
    Internal(String),
}

/// 播放源解析接缝（引擎唯一网络入口）。
///
/// 返回 `BoxFuture`（而非 RPITIT）：RPITIT 的 `impl Future + Send` 返回类型
/// 会使 trait 失去 dyn 兼容性（E0038），而引擎以 `Arc<dyn SourceResolver>`
/// 持有本接缝（见计划 Task 2 Step 3 的备选说明）。
pub trait SourceResolver: Send + Sync {
    /// 解析源为 TrackId 列表（单曲=1 个；歌单/专辑=分页拉取）。
    fn resolve_source_ids(
        &self,
        src: &hmp_core::PlayRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TrackId>, EngineError>> + Send + '_>>;

    /// 解析单曲为可播放 URI + 元数据（音质回退 + QMC2 解密）。
    fn resolve_track(
        &self,
        track_id: &TrackId,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>>;
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
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TrackId>, EngineError>> + Send + '_>> {
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

    // 音质回退链：文档化链（docs/PROJECT.md §7.3，final review Finding 3）。
    // 显式枚举而非 `AudioQuality::Master.fallback_chain()`：后者漏掉 Atmos。
    // 裁决：Aac 不入链（文档化链不含 Aac）。
    const CHAIN: [AudioQuality; 6] = [
        AudioQuality::Master,
        AudioQuality::HiRes,
        AudioQuality::Atmos,
        AudioQuality::Flac,
        AudioQuality::Mp3_320,
        AudioQuality::Mp3_128,
    ];
    let file_info = SongFileInfo {
        mid: track_id.as_ref().to_owned(),
        file_type: None,
        song_type: 0,
        media_mid: Some(media_mid),
    };
    let mut last_error = None;
    for quality in CHAIN {
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
                qualities: vec![quality_from_file_type(&file_type)],
            };
            return Ok(ResolvedTrack { track, uri, media });
        }
    }
    Err(EngineError::QualityUnavailable(
        last_error.unwrap_or_default(),
    ))
}

/// 解析源为 TrackId 列表（单曲/歌单/专辑；歌单/专辑分页拉取，上限 3 页防超限）。
pub async fn resolve_source_ids_impl(
    client: &QqMusicClient,
    src: &hmp_core::PlayRequest,
) -> Result<Vec<TrackId>, EngineError> {
    match src {
        hmp_core::PlayRequest::Track(id) => Ok(vec![id.clone()]),
        hmp_core::PlayRequest::Playlist(id) => {
            let list_id: i64 = id
                .as_ref()
                .parse()
                .map_err(|_| EngineError::PlaylistNotFound("歌单 id 非数字".into()))?;
            let api = SonglistApi::new(client);
            let mut out = Vec::new();
            for page in 1..=3 {
                let resp = api
                    .get_detail(list_id, 0, 100, page, true, false, false)
                    .await
                    .map_err(|e| EngineError::PlaylistNotFound(e.to_string()))?;
                for s in &resp.songs {
                    if !s.mid.is_empty() {
                        out.push(TrackId::new(s.mid.clone()));
                    }
                }
                if resp.hasmore == 0 || out.len() as i64 >= resp.total {
                    break;
                }
            }
            if out.is_empty() {
                return Err(EngineError::PlaylistNotFound("歌单为空".into()));
            }
            Ok(out)
        }
        hmp_core::PlayRequest::Album(id) => {
            let api = AlbumApi::new(client);
            let mut out = Vec::new();
            for page in 1..=3 {
                let resp = api
                    .get_song(id.as_ref(), 100, page)
                    .await
                    .map_err(|e| EngineError::PlaylistNotFound(e.to_string()))?;
                for s in &resp.song_list {
                    if !s.mid.is_empty() {
                        out.push(TrackId::new(s.mid.clone()));
                    }
                }
                if out.len() as i64 >= resp.total_num {
                    break;
                }
            }
            if out.is_empty() {
                return Err(EngineError::PlaylistNotFound("专辑为空".into()));
            }
            Ok(out)
        }
    }
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
