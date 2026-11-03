//! 播放器领域（docs/PROJECT.md §4.3 / §8）。
//!
//! 播放状态**单一来源**：UI 与 MPRIS 都只消费 [`PlaybackState`]，
//! 命令统一经 [`PlayerCommand`] 下发，禁止各方自行推算进度。

use serde::{Deserialize, Serialize};

use crate::HmpError;
use crate::id::TrackId;
use crate::media::{AudioQuality, Track};

/// 播放器可消费的同步媒体字节流（`Read + Seek`）。
///
/// 由 [`MediaStreamSource::open`] 产出，直接喂给解码器（rodio `Decoder`
/// 约束 `R: Read + Seek`）；实现自带后台预取，`Read` 仅在数据饥饿时阻塞。
pub trait MediaStream: std::io::Read + std::io::Seek + Send {}

impl<T: std::io::Read + std::io::Seek + Send> MediaStream for T {}

/// 进程内随机访问媒体源（播放驱动的本地字节流接缝）。
///
/// QQ 远端音频（加密/明文）经 daemon 内的 hmp-media 解密源实现本 trait
/// 直连播放器，取代历史上的回环 HTTP 代理（2026-08 为 GStreamer 引入，
/// gst 弃用后纯属自我强加的 TCP/HTTP/临时文件开销与系统代理劫持事故面）。
///
/// `open` 可重复调用（回滚重装载）；需在 tokio 上下文中调用
/// （后台预取任务挂在当前 runtime 上）。
pub trait MediaStreamSource: Send + Sync + std::fmt::Debug {
    /// 明文总字节数。
    fn len(&self) -> u64;

    /// 是否为空源（默认按 `len() == 0` 判定）。
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 打开一个新的同步 reader。
    fn open(&self) -> std::io::Result<Box<dyn MediaStream>>;
}

/// 播放状态机（docs/PROJECT.md §8.1）。
///
/// 状态转换只能发生在播放器核心（`hmp-player`）中。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackStatus {
    /// 无加载内容。
    Empty,
    /// 正在加载（取流/设置 URI）。
    Loading,
    /// 缓冲中。
    Buffering,
    /// 播放中。
    Playing,
    /// 已暂停。
    Paused,
    /// 已停止。
    Stopped,
    /// 播放到结尾。
    Ended,
    /// 出错。
    Error,
}

/// 循环模式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopMode {
    /// 顺序播放，播完停止。
    #[default]
    None,
    /// 列表循环。
    List,
    /// 单曲循环。
    Track,
}

/// 播放器当前状态（不可变快照，由 `watch` 发布）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaybackState {
    /// 状态机状态。
    pub status: PlaybackStatus,
    /// 当前曲目。
    pub current: Option<Track>,
    /// 播放位置。
    pub position: std::time::Duration,
    /// 总时长。
    pub duration: Option<std::time::Duration>,
    /// 音量（0.0..=1.0）。
    pub volume: f64,
    /// 用户音量（RG 补偿前的原值；`volume` 是叠加 ReplayGain 因子后
    /// 下发驱动的值——UI/MPRIS 展示与回设都用本字段，AUDIT §8.12）。
    #[serde(default = "default_user_volume")]
    pub user_volume: f64,
    /// 循环模式。
    pub loop_mode: LoopMode,
    /// 是否随机播放。
    pub shuffle: bool,
    /// 是否支持 Seek。
    pub can_seek: bool,
    /// 缓冲进度（0.0..=1.0，None=未缓冲）。
    pub buffering: Option<f64>,
    /// 实际播放音质（本次解析选定档位；媒体库重构 B3）。
    #[serde(default)]
    pub actual_quality: Option<AudioQuality>,
    /// 装载代际：每次 driver 装载递增（engine 分配），事件/状态过滤用。
    #[serde(default)]
    pub load_gen: u64,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Empty,
            current: None,
            position: std::time::Duration::ZERO,
            duration: None,
            volume: 1.0,
            user_volume: 1.0,
            loop_mode: LoopMode::None,
            shuffle: false,
            can_seek: false,
            buffering: None,
            actual_quality: None,
            load_gen: 0,
        }
    }
}

/// `user_volume` 的反序列化缺省（旧 daemon 帧无该字段 → 视作未补偿）。
fn default_user_volume() -> f64 {
    1.0
}

/// 播放控制能力（MPRIS `CanGoNext`/`CanGoPrevious` 等由上层队列核心发布）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackCapabilities {
    /// 是否存在下一首。
    pub can_go_next: bool,
    /// 是否存在上一首。
    pub can_go_previous: bool,
}

/// 后端无关的音频装载请求。
///
/// URI 的解析与解码属于具体播放驱动；应用引擎只负责提供已解析的媒体地址、
/// 领域元数据和用于过滤陈旧事件的装载代际。`stream` 存在时优先于 `uri`
/// （进程内直连）；`uri` 始终保留作元数据/日志/回滚记录。
#[derive(Clone)]
pub struct LoadRequest {
    /// 当前曲目元数据。
    pub track: Track,
    /// `file://` 本地地址，或远端曲目的 CDN url（仅元数据/日志用；
    /// 播放路径由 `stream` 决定）。
    pub uri: String,
    /// 本次实际选定的音质。
    pub quality: AudioQuality,
    /// 引擎分配的装载代际。
    pub load_gen: u64,
    /// 进程内随机访问源（远端曲目直连播放器）；本地文件为 `None`（按 uri 打开）。
    pub stream: Option<std::sync::Arc<dyn MediaStreamSource>>,
}

impl std::fmt::Debug for LoadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadRequest")
            .field("track", &self.track)
            .field("uri", &self.uri)
            .field("quality", &self.quality)
            .field("load_gen", &self.load_gen)
            .field("stream", &self.stream.is_some())
            .finish()
    }
}

/// 播放驱动向应用引擎发布的离散事件。
#[derive(Clone, Debug)]
pub enum PlayerEvent {
    /// 已应用新曲目。
    TrackChanged,
    /// 当前装载代际播放结束。
    PlaybackEnded { load_gen: u64 },
    /// 当前装载代际播放失败。
    Error { load_gen: u64, error: HmpError },
    /// 缓冲进度变化（0.0..=1.0，None 表示结束缓冲）。
    BufferingChanged(Option<f64>),
}

impl PlayerEvent {
    /// 返回需要做陈旧事件过滤的装载代际。
    pub const fn load_gen(&self) -> Option<u64> {
        match self {
            Self::PlaybackEnded { load_gen } | Self::Error { load_gen, .. } => Some(*load_gen),
            Self::TrackChanged | Self::BufferingChanged(_) => None,
        }
    }
}

/// `Duration` 以秒（u64）序列化，便于跨进程传递。
pub mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};

    /// 序列化为秒。
    pub fn serialize<S: Serializer>(d: &std::time::Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    /// 从秒反序列化。
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<std::time::Duration, D::Error> {
        Ok(std::time::Duration::from_secs(Deserialize::deserialize(d)?))
    }
}

/// 播放器命令（docs/PROJECT.md §4.3）。
///
/// UI、MPRIS 与 CLI 统一通过 `mpsc` 下发；命令只描述意图，
/// 具体执行由播放器核心完成。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlayerCommand {
    /// 加载并播放曲目。
    LoadAndPlay(TrackId),
    /// 播放（从当前位置恢复）。
    Play,
    /// 暂停。
    Pause,
    /// 播放/暂停切换。
    TogglePlay,
    /// 停止。
    Stop,
    /// 跳转到指定位置（序列化为秒）。
    Seek(#[serde(with = "duration_secs")] std::time::Duration),
    /// 下一首。
    Next,
    /// 上一首。
    Previous,
    /// 设置音量（0.0..=1.0）。
    SetVolume(f64),
    /// 设置循环模式。
    SetLoopMode(LoopMode),
    /// 设置随机播放。
    SetShuffle(bool),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn playback_state_default_is_empty() {
        let s = PlaybackState::default();
        assert_eq!(s.status, PlaybackStatus::Empty);
        assert!(s.current.is_none());
        assert_eq!(s.position, std::time::Duration::ZERO);
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.loop_mode, LoopMode::None);
        assert!(!s.shuffle);
        assert!(!s.can_seek);
        assert!(s.buffering.is_none());
        assert_eq!(s.load_gen, 0);
    }

    #[test]
    fn capabilities_default_is_false() {
        let caps = PlaybackCapabilities::default();
        assert!(!caps.can_go_next);
        assert!(!caps.can_go_previous);
    }

    #[test]
    fn player_command_roundtrips_through_json() {
        let cmds = vec![
            PlayerCommand::Play,
            PlayerCommand::Pause,
            PlayerCommand::Stop,
            PlayerCommand::LoadAndPlay(TrackId::new("mid-1")),
            PlayerCommand::Seek(std::time::Duration::from_secs(90)),
            PlayerCommand::SetVolume(0.5),
            PlayerCommand::SetLoopMode(LoopMode::Track),
            PlayerCommand::SetShuffle(true),
        ];
        for cmd in cmds {
            let v = serde_json::to_value(&cmd).unwrap();
            let back: PlayerCommand = serde_json::from_value(v).unwrap();
            assert_eq!(back, cmd);
        }
    }

    #[test]
    fn loop_mode_default_is_none() {
        assert_eq!(LoopMode::default(), LoopMode::None);
    }

    #[test]
    fn play_command_serializes_as_tag() {
        let v = json!(PlayerCommand::Pause);
        assert_eq!(v, "Pause");
        // Duration 序列化为秒
        let v = json!(PlayerCommand::Seek(std::time::Duration::from_secs(60)));
        assert_eq!(v, json!({"Seek": 60}));
    }

    #[test]
    fn playback_status_variants_are_distinct() {
        for (i, s) in [
            PlaybackStatus::Empty,
            PlaybackStatus::Loading,
            PlaybackStatus::Buffering,
            PlaybackStatus::Playing,
            PlaybackStatus::Paused,
            PlaybackStatus::Stopped,
            PlaybackStatus::Ended,
            PlaybackStatus::Error,
        ]
        .iter()
        .enumerate()
        {
            for (j, t) in [
                PlaybackStatus::Empty,
                PlaybackStatus::Loading,
                PlaybackStatus::Buffering,
                PlaybackStatus::Playing,
                PlaybackStatus::Paused,
                PlaybackStatus::Stopped,
                PlaybackStatus::Ended,
                PlaybackStatus::Error,
            ]
            .iter()
            .enumerate()
            {
                assert_eq!(s == t, i == j);
            }
        }
    }

    #[test]
    fn player_event_preserves_load_generation() {
        let ended = PlayerEvent::PlaybackEnded { load_gen: 42 };
        assert_eq!(ended.load_gen(), Some(42));

        let changed = PlayerEvent::TrackChanged;
        assert_eq!(changed.load_gen(), None);
    }
}
