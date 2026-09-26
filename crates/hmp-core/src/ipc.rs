//! 跨进程控制协议（Unix socket · 长度前缀 JSON 帧）。
//!
//! 消息类型与 `PlayerCommand` 同居（spec §4.1）；传输层在 hmp-daemon。

use serde::{Deserialize, Serialize};

use crate::id::{AlbumId, PlaylistId, TrackId};
use crate::player::{PlaybackCapabilities, PlaybackState, PlayerCommand};
use crate::queue::{QueueSnapshot, QueueSummary};

/// 单帧最大字节数（含 4 字节长度前缀）。
pub const MAX_FRAME: usize = 1 << 20;

/// 播放源请求（曲目 / 歌单 / 专辑 / 本地）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlayRequest {
    /// 单曲。
    Track(TrackId),
    /// 歌单（由后端拉取曲目列表）。
    Playlist(PlaylistId),
    /// 专辑。
    Album(AlbumId),
    /// 本地文件（id 形如 `local:/绝对路径`；媒体库重构 C1）。
    Local(TrackId),
    /// 本地 SQLite 歌单（playlists 表主键；`playlist:local:<id>`；里程碑 F）。
    LibraryPlaylist(i64),
}

impl PlayRequest {
    /// 是否为无需 QQ 凭证的本地播放源。
    ///
    /// `Track` 仍可能来自媒体库等通用入口，因此不能只按枚举分支判断；
    /// `local:` 身份前缀才是曲目 provider 的稳定判据。
    pub fn is_local_source(&self) -> bool {
        match self {
            Self::Local(_) | Self::LibraryPlaylist(_) => true,
            Self::Track(id) => TrackProvider::from_id(id.as_ref()) == TrackProvider::Local,
            Self::Album(id) => id.as_ref().starts_with("local:"),
            Self::Playlist(_) => false,
        }
    }
}

/// 曲目来源提供方。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackProvider {
    /// QQ 音乐（网络取流）。
    QqMusic,
    /// 本地文件（`file://`）。
    Local,
}

impl TrackProvider {
    /// 依据 id 前缀识别来源（`local:` 前缀 → 本地）。
    pub fn from_id(id: &str) -> Self {
        if let Some(rest) = id.strip_prefix("local:") {
            if !rest.is_empty() {
                return Self::Local;
            }
        }
        Self::QqMusic
    }
}

/// 曲目引用（provider + id，媒体库重构 C1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackRef {
    pub provider: TrackProvider,
    pub id: String,
}

impl TrackRef {
    /// 从播放请求映射。
    pub fn from_play_request(r: &PlayRequest) -> Self {
        match r {
            PlayRequest::Local(id) => Self {
                provider: TrackProvider::Local,
                id: id.0.clone(),
            },
            PlayRequest::Track(id) => Self {
                provider: TrackProvider::from_id(id.as_ref()),
                id: id.0.clone(),
            },
            PlayRequest::Playlist(id) => Self {
                provider: TrackProvider::QqMusic,
                id: id.0.clone(),
            },
            PlayRequest::Album(id) => Self {
                provider: TrackProvider::QqMusic,
                id: id.0.clone(),
            },
            PlayRequest::LibraryPlaylist(_) => Self {
                provider: TrackProvider::Local,
                // 本地歌单是复合源（混排 QQ/本地曲目）：单曲映射无意义，
                // 用占位符（解析路径不经过此映射）。
                id: String::new(),
            },
        }
    }

    /// 本地路径（仅当 provider=Local 且 id 以 `local:` 前缀）。
    pub fn local_path(&self) -> Option<&str> {
        (self.provider == TrackProvider::Local)
            .then(|| self.id.strip_prefix("local:"))
            .flatten()
    }
}

/// 客户端 → 后端请求。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// 清空队列并播放该源。
    Play(PlayRequest),
    /// 清空队列并按显式曲目列表播放（GUI 列表入口：可见列表整表替换 +
    /// `start` 为点击行下标；`Play` 只承载后端可自解析的单一源）。
    PlayList {
        /// 曲目 id 序列（QQ mid / `local:<路径>` 混排）。
        ids: Vec<TrackId>,
        /// 起播下标（越界钳制到末位）。
        start: usize,
    },
    /// 插到当前曲之后并立即播放。
    PlayNext(PlayRequest),
    /// 追加到队尾（不播放）。
    QueueAppend(PlayRequest),
    /// 移除 0 基位置曲目。
    QueueRemove(usize),
    /// 清空队列。`all=false`：保留当前曲（清除待播）；`all=true`：清空并停止。
    QueueClear {
        /// 是否连当前曲一起清空（并停止播放）。
        all: bool,
    },
    /// 查询队列快照。
    Queue,
    /// 分页查询队列（防大队列整包超帧上限；元数据投影在客户端侧经媒体库批量查询）。
    QueueList {
        /// 起始偏移。
        offset: usize,
        /// 页大小。
        limit: usize,
    },
    /// 基础播放器命令（Play/Pause/Stop/Seek/Volume/Loop/Shuffle/Next/Previous）。
    Command(PlayerCommand),
    /// 查询全量状态。
    Status,
    /// 收藏写操作（本地先提交；QQ 由 SyncWorker 异步同步）。
    Favorite {
        /// 来源：`qq` | `local`。
        source: String,
        /// 来源身份：QQ mid / `local:<path>`。
        key: String,
        /// 标题（未知时用 id）。
        title: String,
        /// 收藏 / 取消。
        desired: bool,
    },
    /// 歌单写操作（local 直接生效；qq owned 进 outbox）。
    PlaylistWrite {
        /// 操作。
        op: PlaylistWriteOp,
    },
    /// 触发 QQ 用户库 reconcile（library sync；无凭证 → NotLoggedIn）。
    LibrarySync,
    /// 评论查询（读；daemon 内存 TTL cache）。
    CommentList {
        /// 曲目 mid。
        mid: String,
        /// 排序：hot | new | recommend。
        sort: String,
    },
    /// 发表/回复评论（写；直发 QQ）。
    CommentPost {
        /// 曲目 mid。
        mid: String,
        /// 评论内容。
        content: String,
        /// 被回复评论 id（非空即回复）。
        reply_cmt_id: Option<String>,
    },
    /// 删除评论（写）。
    CommentDelete {
        /// 评论 id。
        cm_id: String,
    },
    /// 订阅状态事件流（推送 `Event` 帧）。
    Subscribe,
    /// 播放 URI（MPRIS `OpenUri`；`file://` → 本地，其余 → 内部错误）。
    OpenUri(String),
    /// 跳到队列 0 基位置曲目播放（不替换队列；AUDIT §8.8）。
    /// 曲目级凭证拦截与 Play 同语义（QQ 曲目在 resolve_track 时判）。
    QueuePlayAt(usize),
    /// 快速搜索（免登录 smartbox；daemon 统一出网，AUDIT §8.2）。
    Search {
        /// 关键词。
        keyword: String,
    },
    /// 歌词读取（LRC 文本 + 翻译；daemon 出网并解析 song_type，AUDIT §8.3）。
    LyricGet {
        /// 曲目 mid。
        mid: String,
    },
    /// 账号状态读（登录态 + 昵称/uin/VIP 摘要；AUDIT §8.6）。
    AccountStatus,
    /// 音质偏好读（config.toml `[quality]`）。
    QualityGet,
    /// 音质偏好写（daemon 落 config.toml；UI 只发意图，AUDIT §8.7）。
    QualitySet {
        /// `"auto"` 或音质别名（`master`/`hires`/`atmos`/`flac`/`aac`/`320`/`128`）。
        mode: String,
        /// 是否允许降级回退。
        fallback: bool,
    },
    /// QQ 封面取本地产物：daemon 下载进 `<data_dir>/covers/` 缓存，
    /// 返回 `file://` 路径（UI 禁 HTTP；AUDIT §8.4）。
    CoverGet {
        /// 远程封面 URL。
        url: String,
    },
    /// 发现页聚合：推荐歌单 + 新歌（免登录；daemon 出网，首页内容的基础）。
    DiscoverGet {
        /// 歌单广场页号（1 基）。
        songlist_page: u32,
        /// 新歌地区类型（1=内地 2=欧美 3=日本 4=韩国 5=最新 6=港台）。
        new_song_type: u32,
    },
    /// 排行榜分类（免登录；分组 + 各榜预览前 3 首）。
    TopCategoryGet,
    /// 排行榜详情（免登录；含完整曲目列表）。
    TopDetailGet {
        /// 榜单 ID（来自分类响应）。
        top_id: i64,
        /// 每页数量。
        num: i64,
        /// 页号（1 基）。
        page: i64,
    },
    /// 猜你喜欢（需登录；非安卓平台匿名返回 1000）。
    GuessGet {
        /// 页号（1 基）。
        page: u32,
    },
    /// 优雅退出后端。
    Quit,
}

/// 后端 → 客户端响应。
///
/// `Status(DaemonState)` 较大（含完整播放状态与队列快照），与单位变体并存
/// 属协议设计使然；跨进程按值传递，禁 box 化以免破坏锁定签名。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum Response {
    /// 命令已受理（命令-查询分离，真实结果经 `Event` 呈现）。
    Ok,
    /// `Status` 的响应。
    Status(DaemonState),
    /// `Queue` 的响应。
    Queue(QueueSnapshot),
    /// `QueueList` 的响应。
    QueueList(QueuePage),
    /// 错误。
    Err { code: IpcErrorCode, message: String },
    /// 写操作创建的资源 id（playlist create 等）。
    Created(i64),
    /// `CommentList` 的响应。
    CommentList(CommentPage),
    /// `Search` 的响应。
    Search(SearchPage),
    /// `LyricGet` 的响应。
    Lyric(LyricPage),
    /// `AccountStatus` 的响应。
    AccountStatus(AccountInfo),
    /// `QualityGet` / `QualitySet` 的响应。
    Quality(QualityPrefDto),
    /// `CoverGet` 的响应（`file://` 本地路径）。
    Cover(String),
    /// `DiscoverGet` 的响应（推荐歌单 + 新歌）。
    Discover(DiscoverPage),
    /// `TopCategoryGet` 的响应。
    TopCategory(TopCategoryPage),
    /// `TopDetailGet` 的响应。
    TopDetail(TopDetailPage),
    /// `GuessGet` 的响应。
    Guess(GuessPage),
}

/// 订阅后的事件推送。
///
/// `StateChanged(DaemonState)` 较大（含完整播放状态快照），与轻量单位
/// 变体并存属协议设计使然（跨进程按值传递；禁 box 化破坏锁定签名）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum Event {
    /// 复合状态变更（初始订阅即推一次当前快照）。
    StateChanged(DaemonState),
    /// 媒体库内容变更（扫描/监听/reconcile/写命令落库后触发；
    /// 客户端按直读契约重查 sqlite，AUDIT §8.9）。
    LibraryChanged,
}

/// 后端复合状态（单一状态出口，spec §4.2 `daemon.rs`）。
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct DaemonState {
    /// 播放器状态。
    pub playback: PlaybackState,
    /// 队列摘要（O(1)；完整内容经 QueueList / queue watch）。
    pub queue: QueueSummary,
    /// 播放能力（can_go_next 等）。
    pub caps: PlaybackCapabilities,
    /// 命令代际：换曲操作（Play/PlayNext/Next/Previous）执行前置位，
    /// CLI 据此建立「命令已处理」边界（spec §6；final review Finding 1）。
    pub seq: u64,
    /// 最近一次命令的错误（解析失败等；成功操作时清空，Finding 2）。
    pub last_error: Option<ErrorInfo>,
    /// 当前曲 ReplayGain 标签增益（dB，未 clamp 原值；无标签/QQ 曲目 → None）。
    /// 打磨：CLI status 展示用（MPRIS 无 RG 标准字段，不做非标扩展）。
    pub replaygain_db: Option<f64>,
    /// 播放引擎阶段。
    pub phase: EnginePhase,
}

/// 歌单写操作（本地先提交 + outbox；spec §3.3/§5）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlaylistWriteOp {
    /// 新建本地歌单。
    Create {
        /// 名称。
        name: String,
    },
    /// 重命名（local 直接生效；owned 不支持远端重命名）。
    Rename {
        /// 歌单 id。
        id: i64,
        /// 新名称。
        name: String,
    },
    /// 删除（owned：远端 DelPlaylist 成功后才删本地行）。
    Delete {
        /// 歌单 id。
        id: i64,
    },
    /// 追加曲目（owned：本地提交 + playlist_ops outbox）。
    AddTrack {
        /// 歌单 id。
        id: i64,
        /// 来源：`qq` | `local`。
        source: String,
        /// 来源身份。
        key: String,
        /// 标题。
        title: String,
    },
    /// 按序号移除曲目（owned：outbox）。
    RemoveTrack {
        /// 歌单 id。
        id: i64,
        /// 0 基序号。
        position: i64,
    },
}

/// 评论条目（展示投影；daemon 经 mid→qq_song_id 解析后返回）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommentItem {
    /// 评论 id（回复/删除用；QQ `CmId`）。
    pub cm_id: String,
    /// 分页游标（QQ `SeqNo`）。
    pub seq_no: String,
    pub content: String,
    pub nickname: String,
    /// 时间戳（秒）。
    pub time: i64,
    pub like_count: i64,
}

/// 评论页。
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct CommentPage {
    /// 评论总数。
    pub total: i64,
    pub comments: Vec<CommentItem>,
}

/// 快速搜索单曲结果（smartbox 窄投影）。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SearchSong {
    /// songmid。
    pub mid: String,
    /// 歌曲名。
    pub name: String,
    /// 歌手名（单一展示串）。
    pub singer: String,
}

/// 快速搜索专辑结果。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SearchAlbum {
    /// albummid。
    pub mid: String,
    pub name: String,
    /// 歌手名。
    pub singer: String,
}

/// 快速搜索歌手结果。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SearchSinger {
    /// singermid。
    pub mid: String,
    pub name: String,
}

/// 搜索页（歌曲/专辑/歌手三组）。
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct SearchPage {
    pub songs: Vec<SearchSong>,
    pub albums: Vec<SearchAlbum>,
    pub singers: Vec<SearchSinger>,
}

/// 发现页歌单卡片（推荐歌单广场项的窄投影）。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DiscoverPlaylist {
    /// 歌单数字 ID（播放入口 `hmp play playlist:<id>`）。
    pub id: i64,
    /// 标题。
    pub title: String,
    /// 封面 URL（UI 经 CoverGet 换本地产物）。
    pub picurl: String,
    /// 创建者昵称。
    pub creator: String,
    /// 歌曲数。
    pub songnum: i64,
    /// 播放数。
    pub listennum: i64,
}

/// 发现页新歌项（窄投影：播放所需最小集）。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DiscoverNewSong {
    /// songmid。
    pub mid: String,
    /// 歌曲名。
    pub name: String,
    /// 歌手名（单一展示串）。
    pub singer: String,
    /// 专辑名。
    pub album: String,
    /// 时长秒。
    pub interval: i64,
    /// 封面 URL。
    pub picurl: String,
}

/// 发现页（推荐歌单 + 新歌两大区块）。
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct DiscoverPage {
    /// 推荐歌单（歌单广场）。
    pub playlists: Vec<DiscoverPlaylist>,
    /// 是否还有更多歌单。
    pub has_more_playlists: bool,
    /// 新歌违（按地区类型）。
    pub new_songs: Vec<DiscoverNewSong>,
}

/// 排行榜分组内的榜单摘要（含预览前 3 首）。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TopSummaryDto {
    /// 榜单 ID。
    pub id: i64,
    /// 榜名。
    pub name: String,
    /// 副标题（如期号）。
    pub title_sub: String,
    /// 更新时间。
    pub update_time: String,
    /// 播放数。
    pub listen_num: i64,
    /// 封面 URL。
    pub picurl: String,
    /// 预览前 3 首名次列表（“1. 歌名 - 歌手”）。
    pub preview: Vec<String>,
}

/// 排行榜分组。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TopGroupDto {
    /// 分组 ID。
    pub id: i64,
    /// 分组名（如“热门榜单”）。
    pub name: String,
    /// 组内榜单。
    pub tops: Vec<TopSummaryDto>,
}

/// 排行榜分类页。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TopCategoryPage {
    pub groups: Vec<TopGroupDto>,
}

/// 排行榜详情页曲目。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TopSongDto {
    /// songmid。
    pub mid: String,
    /// 歌曲名。
    pub name: String,
    /// 歌手名（单一展示串）。
    pub singer: String,
    /// 专辑名。
    pub album: String,
    /// 时长秒。
    pub interval: i64,
    /// 封面 URL。
    pub picurl: String,
}

/// 排行榜详情页。
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct TopDetailPage {
    /// 榜名。
    pub name: String,
    /// 副标题/期号。
    pub title_sub: String,
    /// 更新时间。
    pub update_time: String,
    /// 曲目（当前页）。
    pub songs: Vec<TopSongDto>,
    /// 总曲目数。
    pub total: i64,
    /// 是否还有更多页。
    pub has_more: bool,
}

/// 猜你喜欢页（需登录）。
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct GuessPage {
    /// 曲目（窄投影同发现页新歌）。
    pub songs: Vec<DiscoverNewSong>,
}

/// 歌词页（原始 LRC 文本；解析在客户端——桌面 lyrics.rs 复用）。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LyricPage {
    /// 原始歌词（LRC 文本，daemon 侧已解密 QRC）。
    pub lyric: String,
    /// 翻译歌词（LRC 文本；无翻译为空）。
    pub translation: String,
}

/// 账号状态（展示投影；未登录时仅 `logged_in=false` 有意义）。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AccountInfo {
    /// 是否已登录（凭证存在且有效）。
    pub logged_in: bool,
    /// 昵称（拉取失败回退 "QQ {uin}"）。
    pub nickname: String,
    /// QQ 号。
    pub uin: String,
    /// VIP 摘要（如 "VIP 至 2027-01-01"；拉取失败为空）。
    pub vip_summary: String,
}

/// 音质偏好（config.toml `[quality]` 的 IPC 形态）。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QualityPrefDto {
    /// `"auto"` 或音质别名。
    pub mode: String,
    /// 是否允许降级回退。
    pub fallback: bool,
}

/// 队列分页条目（纯 ID + 位置；标题/歌手由客户端经媒体库批量投影，
/// 不在 IPC 里搬运完整 rich metadata）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueEntry {
    /// 曲目 ID。
    pub track_id: TrackId,
    /// 是否当前播放曲。
    pub is_current: bool,
}

/// 队列分页响应。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueuePage {
    /// 队列总曲目数。
    pub total: usize,
    /// 本页起始偏移。
    pub offset: usize,
    /// 本页条目。
    pub items: Vec<QueueEntry>,
}

/// 播放引擎阶段（spec §7 显式状态机：Resolving → Loading → Playing/Failed）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EnginePhase {
    /// 无活动。
    #[default]
    Idle,
    /// 源解析中（歌单/专辑分页拉取）。
    Resolving,
    /// 曲目装载中（解析 + 取流 + 驱动应用）。
    Loading,
    /// 正在播放。
    Playing,
    /// 最近一次装载失败（旧曲/队列保持原状）。
    Failed,
}

/// 最近一次命令的失败详情（final review Finding 2）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// 映射后的 IPC 错误码。
    pub code: IpcErrorCode,
    /// 人类可读错误信息。
    pub message: String,
}

/// 错误码。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IpcErrorCode {
    /// 未登录或凭证失效。
    NotLoggedIn,
    /// 曲目不存在。
    TrackNotFound,
    /// 歌单不存在或拉取失败。
    PlaylistNotFound,
    /// 所有音质均不可用。
    QualityUnavailable,
    /// 协议错误（畸形帧等）。
    BadRequest,
    /// 内部错误。
    Internal,
}

/// 帧编解码错误。
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame length {0} exceeds the limit of {MAX_FRAME}")]
    TooLarge(usize),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// 编码为一帧：`u32 LE 长度 + JSON 字节`。
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let payload = serde_json::to_vec(msg)?;
    let total = payload.len() + 4;
    if total > MAX_FRAME {
        return Err(FrameError::TooLarge(total));
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// 解码一帧（含 4 字节长度前缀；长度超限或前缀与内容不符 → Err）。
pub fn decode_frame<T: serde::de::DeserializeOwned>(frame: &[u8]) -> Result<T, FrameError> {
    if frame.len() < 4 {
        return Err(FrameError::Json(serde_json::Error::io(
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "frame shorter than the 4-byte length prefix",
            ),
        )));
    }
    let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
    if len > MAX_FRAME || 4 + len != frame.len() {
        return Err(FrameError::Json(serde_json::Error::io(
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "frame length prefix does not match payload",
            ),
        )));
    }
    serde_json::from_slice(&frame[4..]).map_err(FrameError::Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{AlbumId, PlaylistId, TrackId};
    use crate::player::PlayerCommand;

    #[test]
    fn request_roundtrips_through_frame() {
        let reqs = vec![
            Request::Play(PlayRequest::Track(TrackId::new("m1"))),
            Request::Play(PlayRequest::Playlist(PlaylistId::new("p1"))),
            Request::Play(PlayRequest::Album(AlbumId::new("a1"))),
            Request::QueueAppend(PlayRequest::Track(TrackId::new("m2"))),
            Request::QueueRemove(2),
            Request::QueueClear { all: false },
            Request::QueueClear { all: true },
            Request::QueueList {
                offset: 1,
                limit: 2,
            },
            Request::Favorite {
                source: "qq".into(),
                key: "k".into(),
                title: "t".into(),
                desired: true,
            },
            Request::PlaylistWrite {
                op: PlaylistWriteOp::Create { name: "n".into() },
            },
            Request::PlaylistWrite {
                op: PlaylistWriteOp::Delete { id: 3 },
            },
            Request::LibrarySync,
            Request::CommentList {
                mid: "m".into(),
                sort: "hot".into(),
            },
            Request::CommentPost {
                mid: "m".into(),
                content: "c".into(),
                reply_cmt_id: Some("r".into()),
            },
            Request::CommentDelete { cm_id: "c".into() },
            Request::Queue,
            Request::Command(PlayerCommand::Seek(std::time::Duration::from_secs(30))),
            Request::Status,
            Request::Subscribe,
            Request::QueuePlayAt(3),
            Request::Search {
                keyword: "夜曲".into(),
            },
            Request::LyricGet { mid: "m".into() },
            Request::AccountStatus,
            Request::QualityGet,
            Request::QualitySet {
                mode: "flac".into(),
                fallback: true,
            },
            Request::CoverGet {
                url: "https://y.gtimg.cn/a.jpg".into(),
            },
            Request::Quit,
        ];
        for req in reqs {
            let frame = encode_frame(&req).unwrap();
            let back: Request = decode_frame(&frame).unwrap();
            assert_eq!(back, req);
        }
    }

    #[test]
    fn library_playlist_roundtrips() {
        let req = PlayRequest::LibraryPlaylist(7);
        let frame = encode_frame(&req).unwrap();
        let back: PlayRequest = decode_frame(&frame).unwrap();
        assert_eq!(back, PlayRequest::LibraryPlaylist(7));
    }

    #[test]
    fn daemon_state_roundtrips() {
        let st = DaemonState {
            playback: Default::default(),
            queue: crate::queue::QueueSummary::default(),
            caps: Default::default(),
            seq: 7,
            last_error: Some(ErrorInfo {
                code: IpcErrorCode::TrackNotFound,
                message: "曲目不存在".into(),
            }),
            replaygain_db: Some(-6.5),
            phase: EnginePhase::Playing,
        };
        let frame = encode_frame(&st).unwrap();
        let back: DaemonState = decode_frame(&frame).unwrap();
        assert_eq!(back, st);
    }

    /// 新响应变体（Created / CommentList）往返序列化。
    #[test]
    fn response_roundtrips_through_frame() {
        let page = CommentPage {
            total: 1,
            comments: vec![CommentItem {
                cm_id: "c".into(),
                seq_no: "s".into(),
                content: "x".into(),
                nickname: "n".into(),
                time: 1,
                like_count: 2,
            }],
        };
        let resps = vec![
            Response::Ok,
            Response::Created(42),
            Response::CommentList(page.clone()),
            Response::Err {
                code: IpcErrorCode::Internal,
                message: "boom".into(),
            },
        ];
        for resp in resps {
            let frame = encode_frame(&resp).unwrap();
            let back: Response = decode_frame(&frame).unwrap();
            assert_eq!(back, resp);
        }
    }

    /// M8 后续新增的读响应与事件往返（Search/Lyric/AccountStatus/Quality/
    /// Cover/LibraryChanged）。
    #[test]
    fn extended_read_responses_and_events_roundtrip() {
        let search = SearchPage {
            songs: vec![SearchSong {
                mid: "0039MnYb0qxYhV".into(),
                name: "夜曲".into(),
                singer: "周杰伦".into(),
            }],
            albums: vec![SearchAlbum {
                mid: "a".into(),
                name: "十一月的萧邦".into(),
                singer: "周杰伦".into(),
            }],
            singers: vec![SearchSinger {
                mid: "s".into(),
                name: "周杰伦".into(),
            }],
        };
        let resps = vec![
            Response::Search(search),
            Response::Lyric(LyricPage {
                lyric: "[00:01.00]test".into(),
                translation: String::new(),
            }),
            Response::AccountStatus(AccountInfo {
                logged_in: true,
                nickname: "胡桃".into(),
                uin: "10001".into(),
                vip_summary: "VIP".into(),
            }),
            Response::Quality(QualityPrefDto {
                mode: "auto".into(),
                fallback: true,
            }),
            Response::Cover("file:///home/u/.local/share/hmp/covers/abc.jpg".into()),
        ];
        for resp in resps {
            let frame = encode_frame(&resp).unwrap();
            let back: Response = decode_frame(&frame).unwrap();
            assert_eq!(back, resp);
        }
        let ev = Event::LibraryChanged;
        let frame = encode_frame(&ev).unwrap();
        let back: Event = decode_frame(&frame).unwrap();
        assert_eq!(back, Event::LibraryChanged);
    }

    #[test]
    fn queue_summary_roundtrips() {
        let s = crate::queue::QueueSummary {
            revision: 9,
            len: 3,
            current: Some(1),
            loop_mode: crate::player::LoopMode::Track,
            shuffle: true,
        };
        let frame = encode_frame(&s).unwrap();
        let back: crate::queue::QueueSummary = decode_frame(&frame).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn frame_prefix_is_u32_le_length() {
        let msg = Request::Status;
        let frame = encode_frame(&msg).unwrap();
        assert_eq!(&frame[..4], &(frame.len() as u32 - 4).to_le_bytes());
    }

    #[test]
    fn frame_size_limit() {
        let big = Request::QueueAppend(PlayRequest::Track(TrackId::new(
            "x".repeat(2 * 1024 * 1024),
        )));
        assert!(encode_frame(&big).is_err());
    }

    #[test]
    fn truncated_frame_rejected() {
        let msg = Request::Status;
        let frame = encode_frame(&msg).unwrap();
        assert!(decode_frame::<Request>(&frame[..frame.len() - 2]).is_err());
    }

    #[test]
    fn local_play_request_roundtrip() {
        // PlayRequest::Local 序列化 round-trip + provider 识别。
        let msg = Request::Play(PlayRequest::Local(TrackId::new("local:/tmp/x.mp3")));
        let frame = encode_frame(&msg).unwrap();
        let back: Request = decode_frame(&frame).unwrap();
        assert_eq!(back, msg);

        assert_eq!(
            TrackProvider::from_id("local:/tmp/x.mp3"),
            TrackProvider::Local
        );
        assert_eq!(TrackProvider::from_id("mid123"), TrackProvider::QqMusic);
        assert_eq!(TrackProvider::from_id("local:"), TrackProvider::QqMusic);

        let r = TrackRef::from_play_request(&PlayRequest::Local(TrackId::new("local:/a.mp3")));
        assert_eq!(r.provider, TrackProvider::Local);
        assert_eq!(r.local_path(), Some("/a.mp3"));
        let r = TrackRef::from_play_request(&PlayRequest::Track(TrackId::new("m")));
        assert_eq!(r.provider, TrackProvider::QqMusic);
        assert_eq!(r.local_path(), None);
        let r = TrackRef::from_play_request(&PlayRequest::Track(TrackId::new("local:/a.mp3")));
        assert_eq!(r.provider, TrackProvider::Local);
        assert_eq!(r.local_path(), Some("/a.mp3"));
    }

    #[test]
    fn play_request_classifies_every_local_source() {
        assert!(PlayRequest::Local(TrackId::new("local:/a.mp3")).is_local_source());
        assert!(PlayRequest::Track(TrackId::new("local:/a.mp3")).is_local_source());
        assert!(PlayRequest::Album(AlbumId::new("local:本地专辑")).is_local_source());
        assert!(PlayRequest::LibraryPlaylist(1).is_local_source());
        assert!(!PlayRequest::Track(TrackId::new("qq-mid")).is_local_source());
        assert!(!PlayRequest::Album(AlbumId::new("qq-album")).is_local_source());
    }
}
