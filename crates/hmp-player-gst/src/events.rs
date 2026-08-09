//! 播放器离散事件（`broadcast` 发布，docs/PROJECT.md §4.3）。

use hmp_core::HmpError;

/// 播放器离散事件。
#[derive(Clone, Debug)]
pub enum PlayerEvent {
    /// 已加载新曲目（URI 已设置）。
    TrackChanged,
    /// 播放到结尾（EOS）；携带装载代际（engine 过滤旧代）。
    PlaybackEnded { load_gen: u64 },
    /// 播放出错；携带装载代际。
    Error { load_gen: u64, error: HmpError },
    /// 缓冲进度变化（0.0..=1.0，None=结束缓冲）。
    BufferingChanged(Option<f64>),
}
