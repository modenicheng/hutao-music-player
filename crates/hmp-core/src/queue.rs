//! 播放队列核心（docs/PROJECT.md §8.4）。
//!
//! 纯逻辑、无 I/O；队列裁决（下一首/上一首/循环/洗牌）唯一实现点，
//! daemon 与未来桌面端共用，禁止在适配器层自行推算。

use serde::{Deserialize, Serialize};

use crate::id::TrackId;
use crate::player::LoopMode;

/// 队列快照（跨进程传递）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QueueSnapshot {
    /// 队列曲目（0 基）。
    pub tracks: Vec<TrackId>,
    /// 当前曲目位置。
    pub current: Option<usize>,
    /// 循环模式。
    pub loop_mode: LoopMode,
    /// 是否洗牌。
    pub shuffle: bool,
}

/// xorshift64*：hmp-core 不引入 rand 依赖，洗牌用自实现 PRNG。
#[derive(Clone, Debug)]
struct XorShift(u64);

impl XorShift {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// 播放队列核心（纯逻辑）。
#[derive(Debug)]
pub struct QueueCore {
    tracks: Vec<TrackId>,
    current: Option<usize>,
    loop_mode: LoopMode,
    shuffle: bool,
    rng: XorShift,
}

impl Default for QueueCore {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueCore {
    /// 空队列。
    pub fn new() -> Self {
        Self {
            tracks: Vec::new(),
            current: None,
            loop_mode: LoopMode::None,
            shuffle: false,
            rng: XorShift(0x9E37_79B9_7F4A_7C15),
        }
    }

    /// 测试/确定性用：注入洗牌种子。
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = XorShift(seed | 1);
    }

    /// 清空并播放 `tracks[start_at]`。
    pub fn replace(&mut self, tracks: Vec<TrackId>, start_at: usize) {
        self.tracks = tracks;
        self.current = if self.tracks.is_empty() {
            None
        } else {
            Some(start_at.min(self.tracks.len() - 1))
        };
    }

    /// 追加到队尾（不改变当前曲）。
    pub fn append(&mut self, tracks: Vec<TrackId>) {
        self.tracks.extend(tracks);
    }

    /// 插到当前曲之后（playnext）。
    pub fn insert_next(&mut self, track: TrackId) {
        let at = self.current.map_or(self.tracks.len(), |i| i + 1);
        self.tracks.insert(at.min(self.tracks.len()), track);
    }

    /// 移除 0 基位置曲目；返回是否成功。
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.tracks.len() {
            return false;
        }
        self.tracks.remove(index);
        if let Some(c) = self.current.as_mut() {
            if index < *c {
                *c -= 1;
            } else if index == *c {
                if self.tracks.is_empty() {
                    self.current = None;
                } else {
                    *c = (*c).min(self.tracks.len() - 1);
                }
            }
        }
        true
    }

    /// 清空队列。
    pub fn clear(&mut self) {
        self.tracks.clear();
        self.current = None;
    }

    /// 当前曲目。
    pub fn current(&self) -> Option<&TrackId> {
        self.current.and_then(|i| self.tracks.get(i))
    }

    /// 快照。
    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            tracks: self.tracks.clone(),
            current: self.current,
            loop_mode: self.loop_mode,
            shuffle: self.shuffle,
        }
    }

    /// 循环模式。
    pub fn loop_mode(&self) -> LoopMode {
        self.loop_mode
    }

    /// 设置循环模式。
    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        self.loop_mode = mode;
    }

    /// 设置洗牌。
    pub fn set_shuffle(&mut self, shuffle: bool) {
        self.shuffle = shuffle;
    }

    /// 洗牌索引：在当前之后的位置里随机选一个；已是队尾时回绕到队首段重抽（排除当前位置）。
    fn shuffled_next(&mut self, from: usize) -> usize {
        let len = self.tracks.len();
        let span = len - from - 1;
        if span == 0 {
            // 队尾无后续：回绕到队首段，仍排除当前位置
            (self.rng.next_u64() % (len - 1) as u64) as usize
        } else {
            from + 1 + (self.rng.next_u64() % span as u64) as usize
        }
    }

    /// 计算并切换到下一首；返回其 TrackId（`None` = 无下一首，引擎保持空闲）。
    pub fn next_track(&mut self) -> Option<TrackId> {
        let len = self.tracks.len();
        if len == 0 {
            return None;
        }
        let cur = self.current.unwrap_or(0);
        if self.shuffle {
            if len <= 1 {
                return Some(self.tracks[cur].clone());
            }
            let next = self.shuffled_next(cur);
            self.current = Some(next);
            return Some(self.tracks[next].clone());
        }
        match self.loop_mode {
            LoopMode::Track => {
                let id = self.tracks[cur].clone();
                Some(id)
            }
            LoopMode::List => {
                let next = (cur + 1) % len;
                self.current = Some(next);
                Some(self.tracks[next].clone())
            }
            LoopMode::None => {
                if cur + 1 >= len {
                    // 到头即停：位置保持，返回 None
                    None
                } else {
                    self.current = Some(cur + 1);
                    Some(self.tracks[cur + 1].clone())
                }
            }
        }
    }

    /// 计算并切换到上一首；`None` = 无上一首（引擎忽略该命令）。
    pub fn prev_track(&mut self) -> Option<TrackId> {
        let len = self.tracks.len();
        if len == 0 {
            return None;
        }
        let cur = self.current.unwrap_or(0);
        match self.loop_mode {
            LoopMode::Track => Some(self.tracks[cur].clone()),
            LoopMode::List => {
                let prev = (cur + len - 1) % len;
                self.current = Some(prev);
                Some(self.tracks[prev].clone())
            }
            LoopMode::None => {
                if cur == 0 {
                    None
                } else {
                    self.current = Some(cur - 1);
                    Some(self.tracks[cur - 1].clone())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::TrackId;
    use crate::player::LoopMode;

    fn t(s: &str) -> TrackId {
        TrackId::new(s)
    }

    #[test]
    fn replace_sets_current_and_tracks() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 1);
        assert_eq!(q.current(), Some(&t("b")));
        let s = q.snapshot();
        assert_eq!(s.tracks, vec![t("a"), t("b"), t("c")]);
        assert_eq!(s.current, Some(1));
    }

    #[test]
    fn next_advances_and_ends_without_loop() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 0);
        assert_eq!(q.next_track(), Some(t("b")));
        assert_eq!(q.next_track(), None); // None 模式到头即停
        assert_eq!(q.current(), Some(&t("b"))); // 位置停在最后一首
    }

    #[test]
    fn list_loop_wraps_around() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 1);
        q.set_loop_mode(LoopMode::List);
        assert_eq!(q.next_track(), Some(t("a"))); // 回绕
    }

    #[test]
    fn track_loop_repeats_current() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 0);
        q.set_loop_mode(LoopMode::Track);
        assert_eq!(q.next_track(), Some(t("a")));
    }

    #[test]
    fn prev_always_jumps_to_previous_track() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 2);
        assert_eq!(q.prev_track(), Some(t("b")));
        assert_eq!(q.prev_track(), Some(t("a")));
        assert_eq!(q.prev_track(), None); // 无上一首（None 模式）
    }

    #[test]
    fn prev_wraps_in_list_mode() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 0);
        q.set_loop_mode(LoopMode::List);
        assert_eq!(q.prev_track(), Some(t("b")));
    }

    #[test]
    fn insert_next_after_current() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 0);
        q.insert_next(t("x"));
        assert_eq!(q.snapshot().tracks, vec![t("a"), t("x"), t("b")]);
        assert_eq!(q.next_track(), Some(t("x")));
    }

    #[test]
    fn remove_adjusts_current() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 1);
        assert!(q.remove(0)); // 删当前之前 → current 前移
        assert_eq!(q.snapshot().current, Some(0));
        assert_eq!(q.snapshot().tracks, vec![t("b"), t("c")]);
        assert!(q.remove(1)); // 删当前之后 → current 不变
        assert_eq!(q.snapshot().current, Some(0));
        assert!(!q.remove(5)); // 越界 → false
    }

    #[test]
    fn append_and_clear() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a")], 0);
        q.append(vec![t("b"), t("c")]);
        assert_eq!(q.snapshot().tracks, vec![t("a"), t("b"), t("c")]);
        q.clear();
        assert_eq!(q.current(), None);
        assert!(q.snapshot().tracks.is_empty());
    }

    #[test]
    fn shuffle_next_excludes_current_and_bounds() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c"), t("d")], 0);
        q.set_shuffle(true);
        // 确定性：注入种子（xorshift 固定种子）
        q.set_seed(42);
        let first = q.next_track().unwrap();
        assert_ne!(first, t("a"));
        let second = q.next_track().unwrap();
        assert_ne!(second, first);
        assert_ne!(second, t("a"));
    }
}
