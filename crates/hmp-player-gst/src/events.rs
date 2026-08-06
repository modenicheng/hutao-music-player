//! 播放器离散事件（`broadcast` 发布，docs/PROJECT.md §4.3）。

use hmp_core::HmpError;

/// 播放器离散事件。
#[derive(Clone, Debug)]
pub enum PlayerEvent {
    /// 已加载新曲目（URI 已设置）。
    TrackChanged,
    /// 播放到结尾（EOS）。
    PlaybackEnded,
    /// 播放出错。
    Error(HmpError),
    /// 缓冲进度变化（0.0..=1.0，None=结束缓冲）。
    BufferingChanged(Option<f64>),
}
