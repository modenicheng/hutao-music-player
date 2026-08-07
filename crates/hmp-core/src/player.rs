//! 播放器领域（docs/PROJECT.md §4.3 / §8）。
//!
//! 播放状态**单一来源**：UI 与 MPRIS 都只消费 [`PlaybackState`]，
//! 命令统一经 [`PlayerCommand`] 下发，禁止各方自行推算进度。

use serde::{Deserialize, Serialize};

use crate::id::TrackId;
use crate::media::Track;

/// 播放状态机（docs/PROJECT.md §8.1）。
///
/// 状态转换只能发生在播放器核心（`hmp-player-gst`）中。
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
    /// 循环模式。
    pub loop_mode: LoopMode,
    /// 是否随机播放。
    pub shuffle: bool,
    /// 是否支持 Seek。
    pub can_seek: bool,
    /// 缓冲进度（0.0..=1.0，None=未缓冲）。
    pub buffering: Option<f64>,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Empty,
            current: None,
            position: std::time::Duration::ZERO,
            duration: None,
            volume: 1.0,
            loop_mode: LoopMode::None,
            shuffle: false,
            can_seek: false,
            buffering: None,
        }
    }
}

/// 播放控制能力（MPRIS `CanGoNext`/`CanGoPrevious` 等由上层队列核心发布）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackCapabilities {
    /// 是否存在下一首。
    pub can_go_next: bool,
    /// 是否存在上一首。
    pub can_go_previous: bool,
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
}
