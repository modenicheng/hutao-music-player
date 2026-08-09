//! 跨进程控制协议（Unix socket · 长度前缀 JSON 帧）。
//!
//! 消息类型与 `PlayerCommand` 同居（spec §4.1）；传输层在 hmp-daemon。

use serde::{Deserialize, Serialize};

use crate::id::{AlbumId, PlaylistId, TrackId};
use crate::player::{PlaybackCapabilities, PlaybackState, PlayerCommand};
use crate::queue::QueueSnapshot;

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
                provider: TrackProvider::QqMusic,
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
}

/// 订阅后的事件推送。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// 复合状态变更（初始订阅即推一次当前快照）。
    StateChanged(DaemonState),
}

/// 后端复合状态（单一状态出口，spec §4.2 `daemon.rs`）。
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct DaemonState {
    /// 播放器状态。
    pub playback: PlaybackState,
    /// 队列快照。
    pub queue: QueueSnapshot,
    /// 播放能力（can_go_next 等）。
    pub caps: PlaybackCapabilities,
    /// 命令代际：换曲操作（Play/PlayNext/Next/Previous）执行前置位，
    /// CLI 据此建立「命令已处理」边界（spec §6；final review Finding 1）。
    pub seq: u64,
    /// 最近一次命令的错误（解析失败等；成功操作时清空，Finding 2）。
    pub last_error: Option<ErrorInfo>,
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
    #[error("帧长度 {0} 超过上限 {MAX_FRAME}")]
    TooLarge(usize),
    #[error("json 错误: {0}")]
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
                "frame 短于 4 字节长度前缀",
            ),
        )));
    }
    let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
    if len > MAX_FRAME || 4 + len != frame.len() {
        return Err(FrameError::Json(serde_json::Error::io(
            std::io::Error::new(std::io::ErrorKind::InvalidData, "帧长度前缀与内容不符"),
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
            Request::Queue,
            Request::Command(PlayerCommand::Seek(std::time::Duration::from_secs(30))),
            Request::Status,
            Request::Subscribe,
            Request::Quit,
        ];
        for req in reqs {
            let frame = encode_frame(&req).unwrap();
            let back: Request = decode_frame(&frame).unwrap();
            assert_eq!(back, req);
        }
    }

    #[test]
    fn daemon_state_roundtrips() {
        let st = DaemonState {
            playback: Default::default(),
            queue: crate::queue::QueueSnapshot::default(),
            caps: Default::default(),
            seq: 7,
            last_error: Some(ErrorInfo {
                code: IpcErrorCode::TrackNotFound,
                message: "曲目不存在".into(),
            }),
            phase: EnginePhase::Playing,
        };
        let frame = encode_frame(&st).unwrap();
        let back: DaemonState = decode_frame(&frame).unwrap();
        assert_eq!(back, st);
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
    }
}
