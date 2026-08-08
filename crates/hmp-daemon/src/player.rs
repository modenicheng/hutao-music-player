//! 播放驱动抽象、曲目解析与解析错误（spec §4.2 `player.rs`）。
//!
//! [`PlaybackDriver`] 是后端与播放器的唯一接缝：测试注入 fake，生产用
//! [`GstDriver`]（包 `PlayerCore`）。[`SourceResolver`] 是后端与 QQ API
//! 的唯一接缝：测试注入 fake，生产用 [`QqSourceResolver`]。队列裁决/
//! 自动续播在引擎（`engine.rs`），播放器核心不感知队列。

use std::future::Future;
use std::pin::Pin;

use hmp_core::{PlaybackState, PlayerCommand, Track, TrackId};
use hmp_player_gst::{LoadRequest, PlayerCore, PlayerEvent};
use hmp_qqmusic_api::QqMusicClient;
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
    // `client` 在 Task 3 的解析实现中使用（音质回退/歌单专辑拉取）。
    #[allow(dead_code)]
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

    #[allow(dead_code)] // Task 3 解析实现（resolve_track_impl/resolve_source_ids_impl）使用
    fn load_credential(&self) -> Result<hmp_storage::credential::Credential, EngineError> {
        self.store
            .load()
            .map_err(|e| EngineError::Internal(format!("读取凭证失败: {e}")))?
            .ok_or(EngineError::NotLoggedIn)
    }
}

// TODO(Task 3): 填充真实解析实现（自由函数 `resolve_source_ids_impl` /
// `resolve_track_impl`，见计划 Task 3 Step 4）。当前占位保证 daemon 组装
// 可编译：引擎运行正常，仅 Play/PlayNext/QueueAppend 的解析阶段返回错误。
impl SourceResolver for QqSourceResolver {
    fn resolve_source_ids(
        &self,
        _src: &hmp_core::PlayRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TrackId>, EngineError>> + Send + '_>> {
        Box::pin(async {
            Err(EngineError::Internal("曲目解析未实现（Task 3）".to_owned()))
        })
    }

    fn resolve_track(
        &self,
        _track_id: &TrackId,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
        Box::pin(async {
            Err(EngineError::Internal("曲目解析未实现（Task 3）".to_owned()))
        })
    }
}
