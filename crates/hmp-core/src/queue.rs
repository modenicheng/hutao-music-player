//! 播放队列核心（docs/PROJECT.md §8.4）。
//!
//! 纯逻辑、无 I/O；队列裁决（下一首/上一首/循环/洗牌）唯一实现点，
//! daemon 与未来桌面端共用，禁止在适配器层自行推算。
//!
//! 播放模型：**规范顺序 + 播放顺序**。`tracks` 是规范（显示）顺序；
//! `order` 是播放顺序（非洗牌 = 恒等排列，洗牌 = 随机排列），`cursor`
//! 是当前曲在 `order` 中的位置。上一首/下一首沿 `order` 移动，因此洗牌
//! 模式下 Previous 回到真正刚播过的曲目（顺序历史由 cursor 天然携带）。
//! `skip_next`（用户主动跳歌）与 `advance_on_eos`（自然播完）语义分离：
//! Repeat One 只影响后者，不阻止主动跳歌。

use serde::{Deserialize, Serialize};

use crate::id::TrackId;
use crate::player::LoopMode;

/// 队列快照（跨进程传递）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QueueSnapshot {
    /// 队列曲目（0 基，规范顺序）。
    pub tracks: Vec<TrackId>,
    /// 当前曲目位置（规范下标）。
    pub current: Option<usize>,
    /// 循环模式。
    pub loop_mode: LoopMode,
    /// 是否洗牌。
    pub shuffle: bool,
}

/// 队列摘要（O(1)，随 DaemonState 发布；完整内容走 QueueList/queue watch）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QueueSummary {
    /// 队列结构版本（每次变更 +1；position tick 不递增）。
    pub revision: u64,
    /// 队列总曲目数。
    pub len: usize,
    /// 当前曲目位置（规范下标）。
    pub current: Option<usize>,
    /// 循环模式。
    pub loop_mode: LoopMode,
    /// 是否洗牌。
    pub shuffle: bool,
}

/// 队列完整内部状态（含播放顺序排列；引擎事务回滚用，见 [`QueueCore::save_state`]）。
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct QueueState {
    pub tracks: Vec<TrackId>,
    pub order: Vec<usize>,
    pub cursor: usize,
    pub has_current: bool,
    pub loop_mode: LoopMode,
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
    /// 规范顺序（快照/显示）。
    tracks: Vec<TrackId>,
    /// 播放顺序排列（`order[i]` = 规范下标；非洗牌 = 0..n）。
    order: Vec<usize>,
    /// 当前曲在 `order` 中的位置。
    cursor: usize,
    /// 是否已有当前曲（区别于 cursor 初始 0）。
    has_current: bool,
    loop_mode: LoopMode,
    shuffle: bool,
    rng: XorShift,
    /// 结构版本（变更方法自动递增；position tick 不递增）。
    revision: u64,
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
            order: Vec::new(),
            cursor: 0,
            has_current: false,
            loop_mode: LoopMode::None,
            shuffle: false,
            rng: XorShift(0x9E37_79B9_7F4A_7C15),
            revision: 0,
        }
    }

    /// 测试/确定性用：注入洗牌种子。
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = XorShift(seed | 1);
    }

    /// 清空并播放 `tracks[start_at]`。
    ///
    /// 不变式：`order` 恒为 `tracks` 的排列且 `cursor` 在其内有效（`has_current` 时）。
    /// 本方法先按**新队列**计算 canonical 当前曲，再重建播放顺序；
    /// 不得依赖旧 `order`（旧队列可能是空的或长度不同的，P0 越界根因）。
    pub fn replace(&mut self, tracks: Vec<TrackId>, start_at: usize) {
        let current = if tracks.is_empty() {
            None
        } else {
            Some(start_at.min(tracks.len() - 1))
        };
        self.tracks = tracks;
        self.has_current = current.is_some();
        self.rebuild_order();
        if let Some(c) = current {
            self.cursor = self.order.iter().position(|&x| x == c).unwrap_or(0);
        } else {
            self.cursor = 0;
        }
        self.revision += 1;
    }

    /// 追加到队尾（不改变当前曲）。
    pub fn append(&mut self, tracks: Vec<TrackId>) {
        if tracks.is_empty() {
            return;
        }
        let base = self.tracks.len();
        self.tracks.extend(tracks);
        for i in base..self.tracks.len() {
            self.order.push(i);
        }
        self.revision += 1;
    }

    /// 把整片曲目插到当前曲之后（playnext 多曲目；顺序保持，非逐条反插）。
    ///
    /// 返回第一个插入位置的规范下标；队列为空时按 `replace` 建队并返回 0。
    pub fn insert_after_current(&mut self, ids: Vec<TrackId>) -> Option<usize> {
        if ids.is_empty() {
            return None;
        }
        if self.tracks.is_empty() {
            self.replace(ids, 0);
            return Some(0);
        }
        let at = if self.has_current {
            self.order[self.cursor] + 1
        } else {
            self.tracks.len()
        };
        let at = at.min(self.tracks.len());
        // 规范顺序：整片插入 at 处。
        self.tracks.splice(at..at, ids.clone());
        // 播放顺序：新曲目的规范下标序列插入 cursor 之后。
        let new_idx: Vec<usize> = (at..at + ids.len()).collect();
        let insert_at = if self.has_current {
            self.cursor + 1
        } else {
            self.order.len()
        };
        let insert_at = insert_at.min(self.order.len());
        self.order.splice(insert_at..insert_at, new_idx);
        // 调整 at 之后原有 order 元素的规范下标（因 splice 已顺移）。
        // splice 只移动了 tracks 中的下标；order 中指向 >= at 的旧元素需要 +ids.len()。
        // 但新插入的规范下标 (at..at+len) 已占位；把插入点之后、属于旧曲目的元素平移。
        // 实现：对 order 中插入点之后且原值 >= at 的条目，在原值已被占用时递增。
        // 简化正确做法：重建 order（保序平移）。
        self.rebuild_order_after_insert(at, ids.len());
        self.revision += 1;
        Some(at)
    }

    /// 移除 0 基规范位置曲目；返回是否成功。
    ///
    /// 移除当前曲时当前曲自动滑到接替曲（原 order 位置）；引擎负责重新加载。
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.tracks.len() {
            return false;
        }
        self.tracks.remove(index);
        // order 中值为 index 的元素即被移除曲目。
        let p = self.order.iter().position(|&v| v == index);
        let removed_cursor_pos = p.unwrap_or(usize::MAX);
        if let Some(p) = p {
            self.order.remove(p);
        }
        for v in self.order.iter_mut() {
            if *v > index {
                *v -= 1;
            }
        }
        if self.has_current {
            if removed_cursor_pos == self.cursor {
                // 当前曲被移除：has_current 保留（cursor 指向接替曲），空队则清。
                if self.order.is_empty() {
                    self.has_current = false;
                } else {
                    self.cursor = self.cursor.min(self.order.len() - 1);
                }
            } else if removed_cursor_pos < self.cursor {
                self.cursor -= 1;
            }
        }
        self.revision += 1;
        true
    }

    /// 清空队列。
    /// 清空队列（连当前曲一起）。
    pub fn clear(&mut self) {
        let changed = !self.tracks.is_empty();
        self.tracks.clear();
        self.order.clear();
        self.cursor = 0;
        self.has_current = false;
        if changed {
            self.revision += 1;
        }
    }

    /// 清除待播曲目，保留当前曲（`queue clear` 语义）：
    /// 不产生「队列已空但 current 正在播」的中间态——当前曲继续播放，
    /// 队列只剩它一首。无当前曲时等价于 [`Self::clear`]。
    pub fn clear_pending(&mut self) {
        let Some(current) = self.current_idx() else {
            self.clear();
            return;
        };
        self.tracks = vec![self.tracks[current].clone()];
        self.order = vec![0];
        self.cursor = 0;
        self.has_current = true;
        self.revision += 1;
    }

    /// 当前曲目（规范顺序视图）。
    pub fn current(&self) -> Option<&TrackId> {
        if self.has_current {
            self.tracks.get(self.order[self.cursor])
        } else {
            None
        }
    }

    /// 当前曲目的规范下标。
    pub fn current_idx(&self) -> Option<usize> {
        self.has_current.then(|| self.order[self.cursor])
    }

    /// 将当前曲定位到指定规范位置（越界钳制到队尾；空队列无操作）。
    pub fn set_current(&mut self, index: usize) {
        if self.tracks.is_empty() {
            return;
        }
        let canonical = index.min(self.tracks.len() - 1);
        if let Some(p) = self.order.iter().position(|&x| x == canonical) {
            self.cursor = p;
            self.has_current = true;
            self.revision += 1;
        }
    }

    /// 当前结构版本（变更方法自动递增；position tick 不递增）。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// O(1) 摘要（随 DaemonState 发布；完整内容经 QueueList/queue watch）。
    pub fn summary(&self) -> QueueSummary {
        QueueSummary {
            revision: self.revision,
            len: self.tracks.len(),
            current: self.current_idx(),
            loop_mode: self.loop_mode,
            shuffle: self.shuffle,
        }
    }

    /// 快照（跨进程）。
    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            tracks: self.tracks.clone(),
            current: self.current_idx(),
            loop_mode: self.loop_mode,
            shuffle: self.shuffle,
        }
    }

    /// 完整内部状态（含播放顺序与 cursor；引擎事务回滚用）。
    #[doc(hidden)]
    pub fn save_state(&self) -> QueueState {
        QueueState {
            tracks: self.tracks.clone(),
            order: self.order.clone(),
            cursor: self.cursor,
            has_current: self.has_current,
            loop_mode: self.loop_mode,
            shuffle: self.shuffle,
        }
    }

    /// 回滚到 `save_state` 捕获的状态（保留 rng 种子）。
    #[doc(hidden)]
    pub fn restore_state(&mut self, s: QueueState) {
        self.tracks = s.tracks;
        self.order = s.order;
        self.cursor = s.cursor;
        self.has_current = s.has_current;
        self.loop_mode = s.loop_mode;
        self.shuffle = s.shuffle;
        self.revision += 1;
    }

    /// 循环模式。
    pub fn loop_mode(&self) -> LoopMode {
        self.loop_mode
    }

    /// 设置循环模式。
    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        self.loop_mode = mode;
        self.revision += 1;
    }

    /// 是否洗牌。
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    /// 设置洗牌：开 → 生成随机播放顺序（当前曲保持在原槽位，位置不跳变）；
    /// 关 → 恢复恒等顺序，**当前曲目（canonical）保持不变**——
    /// 只保留 cursor 数值会把当前曲静默换成另一首（旧实现 bug）。
    pub fn set_shuffle(&mut self, shuffle: bool) {
        if self.shuffle == shuffle {
            return;
        }
        // 旧 order 一致（不变式），此刻读取 canonical 当前曲安全。
        let cur = self.current_idx();
        self.shuffle = shuffle;
        self.rebuild_order();
        if let Some(cur) = cur {
            if let Some(p) = self.order.iter().position(|&x| x == cur) {
                if shuffle {
                    // 开：当前曲保持在原 cursor 槽（播放位置不跳变）。
                    let slot = self.cursor.min(self.order.len() - 1);
                    self.order.swap(p, slot);
                    self.cursor = slot;
                } else {
                    // 关：当前曲 = 原 canonical 曲在新恒等顺序中的位置。
                    self.cursor = p;
                }
            }
        }
        self.revision += 1;
    }

    /// 用户主动下一首。Repeat One **不**阻止跳歌（Track 模式按回绕处理）；
    /// List 循环回绕；**None 模式到头即停**（shuffle 只决定顺序，不隐含列表循环）。
    pub fn skip_next(&mut self) -> Option<TrackId> {
        let out = self.skip_next_inner();
        if out.is_some() {
            self.revision += 1;
        }
        out
    }

    /// `skip_next` 的结构变更内核（revision 由外层统一递增）。
    fn skip_next_inner(&mut self) -> Option<TrackId> {
        if self.tracks.is_empty() {
            return None;
        }
        if !self.has_current {
            self.cursor = 0;
            self.has_current = true;
            return Some(self.tracks[self.order[0]].clone());
        }
        if self.cursor + 1 >= self.order.len() {
            // 随机/恒等序列到头：None 停止；List/Track 回绕。
            match self.loop_mode {
                LoopMode::None => return None,
                _ => self.cursor = 0,
            }
        } else {
            self.cursor += 1;
        }
        Some(self.tracks[self.order[self.cursor]].clone())
    }

    /// 自然播完（EOS）续播。Repeat One 只影响这里：Track 模式重播当前，
    /// 不推进；其余同 `skip_next`。
    pub fn advance_on_eos(&mut self) -> Option<TrackId> {
        if self.tracks.is_empty() {
            return None;
        }
        // Track 模式重播当前：结构未变，revision 不递增（其余路径走 skip_next 递增）。
        if self.loop_mode == LoopMode::Track && self.has_current {
            return Some(self.tracks[self.order[self.cursor]].clone());
        }
        self.skip_next()
    }

    /// 上一首：沿播放顺序回退（洗牌下即回到真正刚播过的那首）。
    /// Repeat One 只影响 EOS 续播，不影响手动 Previous：Track 与 None 一致
    /// （回退、队首即停）；仅 List 回绕。
    pub fn prev_track(&mut self) -> Option<TrackId> {
        let out = self.prev_track_inner();
        if out.is_some() {
            self.revision += 1;
        }
        out
    }

    /// `prev_track` 的结构变更内核（revision 由外层统一递增）。
    fn prev_track_inner(&mut self) -> Option<TrackId> {
        if self.tracks.is_empty() {
            return None;
        }
        if !self.has_current {
            self.cursor = 0;
            self.has_current = true;
            return Some(self.tracks[self.order[0]].clone());
        }
        match self.loop_mode {
            LoopMode::List => {
                self.cursor = (self.cursor + self.order.len() - 1) % self.order.len();
                Some(self.tracks[self.order[self.cursor]].clone())
            }
            LoopMode::Track | LoopMode::None => {
                if self.cursor == 0 {
                    None
                } else {
                    self.cursor -= 1;
                    Some(self.tracks[self.order[self.cursor]].clone())
                }
            }
        }
    }

    /// 播放能力：CanGoNext（Track/List 恒可；None 视位置；shuffle 不额外放行）。
    pub fn can_go_next(&self) -> bool {
        if self.tracks.is_empty() {
            return false;
        }
        matches!(self.loop_mode, LoopMode::Track | LoopMode::List)
            || (self.has_current && self.cursor + 1 < self.order.len())
    }

    /// 播放能力：CanGoPrevious（List 恒可——回绕；Track 与 None 一致，
    /// Repeat One 不放大手动 Previous 能力——视位置）。
    pub fn can_go_previous(&self) -> bool {
        if self.tracks.is_empty() {
            return false;
        }
        matches!(self.loop_mode, LoopMode::List) || (self.has_current && self.cursor > 0)
    }

    /// （重新）生成播放顺序：洗牌 → Fisher-Yates 排列；否则恒等排列。
    /// 注意：**不得**在此读取 `current_idx()`（旧 `order` 可能已失效，P0 根因）；
    /// 需要保留当前曲的调用方应在队列变更前自行计算 canonical 下标。
    fn rebuild_order(&mut self) {
        let n = self.tracks.len();
        if self.shuffle && n > 1 {
            let mut perm: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                let j = (self.rng.next_u64() % (i + 1) as u64) as usize;
                perm.swap(i, j);
            }
            self.order = perm;
        } else {
            self.order = (0..n).collect();
        }
        self.cursor = self.cursor.min(self.order.len().saturating_sub(1));
    }

    /// 插入后的播放顺序修正：`tracks` 中 at..at+len 为新曲（规范下标已就位）；
    /// order 中其余规范下标 >= at 的旧条目需要 +len（因为 tracks 已顺移）。
    /// 新插入条目在 order 中的位置由调用方 splice 决定，此函数只平移旧条目。
    fn rebuild_order_after_insert(&mut self, at: usize, len: usize) {
        // 识别哪些 order 元素属于新插入：其值在 [at, at+len) 且插入点之后。
        // 简单可靠：以当前 order 的插入段（cursor+1 起 len 个）为新元素，
        // 其余值 >= at 的旧元素 +len。
        let new_start = if self.has_current {
            self.cursor + 1
        } else {
            self.order.len().saturating_sub(len)
        };
        for (i, v) in self.order.iter_mut().enumerate() {
            let is_new = i >= new_start && i < new_start + len;
            if !is_new && *v >= at {
                *v += len;
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
        assert_eq!(q.current_idx(), Some(1));
        let s = q.snapshot();
        assert_eq!(s.tracks, vec![t("a"), t("b"), t("c")]);
        assert_eq!(s.current, Some(1));
    }

    #[test]
    fn skip_next_advances_and_ends_without_loop() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 0);
        assert_eq!(q.skip_next(), Some(t("b")));
        assert_eq!(q.skip_next(), None); // None 模式到头即停
        assert_eq!(q.current(), Some(&t("b"))); // 位置停在最后一首
    }

    #[test]
    fn list_loop_wraps_around() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 1);
        q.set_loop_mode(LoopMode::List);
        assert_eq!(q.skip_next(), Some(t("a"))); // 回绕
    }

    #[test]
    fn track_loop_skips_to_next_track() {
        // Repeat One 不阻止主动跳歌：Track 模式下 skip_next 前进到下一首。
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 0);
        q.set_loop_mode(LoopMode::Track);
        assert_eq!(q.skip_next(), Some(t("b")));
    }

    #[test]
    fn track_loop_eos_replays_current() {
        // EOS 续播受 Repeat One 影响：重播当前曲。
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 0);
        q.set_loop_mode(LoopMode::Track);
        assert_eq!(q.advance_on_eos(), Some(t("a")));
        assert_eq!(q.current(), Some(&t("a")));
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
    fn prev_in_track_mode_goes_back_not_replay() {
        // Repeat One 只影响 EOS：Track 模式手动 Previous 回退上一曲，队首即停。
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 2);
        q.set_loop_mode(LoopMode::Track);
        assert_eq!(q.prev_track(), Some(t("b")));
        assert_eq!(q.prev_track(), Some(t("a")));
        assert_eq!(q.prev_track(), None); // 队首：无上一首
        assert!(!q.can_go_previous());
    }

    #[test]
    fn clear_pending_keeps_current_only() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 1); // current = b
        q.clear_pending();
        let snap = q.snapshot();
        assert_eq!(snap.tracks, vec![t("b")]);
        assert_eq!(snap.current, Some(0));
        assert_eq!(q.current(), Some(&t("b")));
        // 无当前曲（空队列）时等价于 clear。
        let mut q2 = QueueCore::new();
        q2.clear_pending();
        assert!(q2.snapshot().tracks.is_empty());
        assert_eq!(q2.snapshot().current, None);
    }

    #[test]
    fn insert_after_current_multi_preserves_order() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 1);
        let pos = q.insert_after_current(vec![t("x"), t("y"), t("z")]);
        assert_eq!(pos, Some(2));
        assert_eq!(
            q.snapshot().tracks,
            vec![t("a"), t("b"), t("x"), t("y"), t("z"), t("c")]
        );
        // 播放顺序：x 紧跟当前（b）之后。
        assert_eq!(q.skip_next(), Some(t("x")));
        assert_eq!(q.skip_next(), Some(t("y")));
        assert_eq!(q.skip_next(), Some(t("z")));
    }

    #[test]
    fn insert_after_current_on_empty_builds_queue() {
        let mut q = QueueCore::new();
        let pos = q.insert_after_current(vec![t("a"), t("b")]);
        assert_eq!(pos, Some(0));
        assert_eq!(q.snapshot().tracks, vec![t("a"), t("b")]);
        assert_eq!(q.current(), Some(&t("a")));
    }

    #[test]
    fn remove_adjusts_current() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 1);
        assert!(q.remove(0)); // 删当前之前 → current 规范下标前移
        assert_eq!(q.snapshot().current, Some(0));
        assert_eq!(q.snapshot().tracks, vec![t("b"), t("c")]);
        assert!(q.remove(1)); // 删当前之后 → current 不变
        assert_eq!(q.snapshot().current, Some(0));
        assert!(!q.remove(5)); // 越界 → false
    }

    #[test]
    fn remove_current_slides_to_replacement() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 1);
        assert!(q.remove(1)); // 删当前（b）→ 接替曲 c
        assert_eq!(q.current(), Some(&t("c")));
        assert_eq!(q.skip_next(), None); // c 是队尾，None 模式到头
        q.set_current(0);
        assert_eq!(q.current(), Some(&t("a")));
    }

    #[test]
    fn remove_current_last_then_empty() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 1);
        assert!(q.remove(1));
        assert_eq!(q.current(), Some(&t("a"))); // 接替曲 a
        assert!(q.remove(0));
        assert_eq!(q.current(), None);
        assert_eq!(q.snapshot().current, None);
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
    fn set_current_clamps() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 0);
        q.set_current(99); // 越界 → 钳制到队尾
        assert_eq!(q.current(), Some(&t("c")));
        let mut empty = QueueCore::new();
        empty.set_current(0); // 空队列 no-op
        assert_eq!(empty.current(), None);
    }

    #[test]
    fn shuffle_visits_all_tracks_without_repeat() {
        // shuffle + None 循环：完整访问随机周期后停止（不隐含列表循环）。
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c"), t("d"), t("e")], 0);
        q.set_seed(42);
        q.set_shuffle(true);
        let first = q.current().cloned().unwrap();
        let mut seen = std::collections::HashSet::new();
        seen.insert(first.clone());
        let mut played = vec![first.clone()];
        for _ in 0..4 {
            let id = q.skip_next().unwrap();
            assert!(seen.insert(id.clone()), "洗牌周期内不应重复 {id}");
            played.push(id);
        }
        // 周期播完（None 循环）→ 停止，不隐式回绕。
        assert_eq!(q.skip_next(), None, "shuffle + None 周期末应停止");
        assert_eq!(played.len(), 5);
    }

    #[test]
    fn shuffle_prev_returns_actually_played() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c"), t("d")], 0);
        q.set_seed(7);
        q.set_shuffle(true);
        let first = q.current().cloned().unwrap();
        let second = q.skip_next().unwrap();
        assert_ne!(first, second);
        // Previous 应回到真正刚播过的 second 之前那首 = first（播放顺序历史）。
        let prev = q.prev_track().unwrap();
        assert_eq!(prev, first);
        // 回到周期起点后（None 循环）Previous 再按无上一首处理（不再隐式回绕）。
        assert_eq!(q.prev_track(), None);
    }

    #[test]
    fn shuffle_toggle_preserves_current() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c"), t("d")], 2);
        q.set_seed(42);
        q.set_shuffle(true);
        assert_eq!(q.current(), Some(&t("c"))); // 洗牌后当前曲不变
        assert_eq!(q.current_idx(), Some(2));
        q.set_shuffle(false);
        assert_eq!(q.current(), Some(&t("c")));
        assert_eq!(q.skip_next(), Some(t("d"))); // 恒等顺序恢复
    }

    #[test]
    fn shuffle_cycle_respects_order_unique() {
        // 确定性：同种子同队列 → 同播放顺序；且是合法排列（5 曲互不重复）。
        let run = |seed: u64| {
            let mut q = QueueCore::new();
            q.replace(vec![t("a"), t("b"), t("c"), t("d"), t("e")], 0);
            q.set_seed(seed);
            q.set_shuffle(true);
            let mut seq = vec![q.current().cloned().unwrap()];
            for _ in 0..4 {
                seq.push(q.skip_next().unwrap());
            }
            seq
        };
        assert_eq!(run(42), run(42));
        let seq = run(42);
        assert_eq!(seq.len(), 5);
        let unique: std::collections::HashSet<_> = seq.iter().collect();
        assert_eq!(unique.len(), 5, "洗牌周期内应为无重复排列");
    }

    #[test]
    fn caps_with_shuffle_always_allow_next() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 2); // 队尾
        assert!(!q.can_go_next()); // None 模式队尾不可 next
        assert!(q.can_go_previous()); // 但 prev 可用（cursor 2 > 0）
        q.set_shuffle(true);
        // shuffle 只改顺序：None 循环队尾仍不可 next（不再因 shuffle 隐含列表循环）。
        assert!(!q.can_go_next());
        assert!(q.can_go_previous());
        q.set_loop_mode(LoopMode::List);
        assert!(q.can_go_next()); // 列表循环恒可
        assert!(q.can_go_previous());
    }

    #[test]
    fn caps_none_mode_limited_by_position() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 1);
        assert!(q.can_go_next());
        assert!(q.can_go_previous());
        q.set_current(0);
        assert!(!q.can_go_previous());
    }

    #[test]
    fn caps_empty_queue() {
        let q = QueueCore::new();
        assert!(!q.can_go_next());
        assert!(!q.can_go_previous());
    }

    /// P0 回归：空队列开 shuffle → replace 多曲目不得越界 panic。
    /// （旧实现 rebuild_order 从已失效的空 order 读 current_idx() → 越界。）
    #[test]
    fn replace_after_empty_with_shuffle_does_not_panic() {
        let mut q = QueueCore::new();
        q.set_shuffle(true); // 空队列：order = []
        q.replace(vec![t("a"), t("b"), t("c")], 0);
        assert_eq!(q.current(), Some(&t("a")));
        assert_eq!(q.snapshot().tracks.len(), 3);
    }

    /// P1：shuffle 播放若干首后关闭 shuffle，当前曲目必须保持不变
    /// （旧实现保留 cursor 数值 → QueueCore 静默换曲，GStreamer 仍播原曲）。
    /// 多种子扫描：任意种子下关 shuffle 后 canonical 当前曲不变（旧实现部分种子失败）。
    #[test]
    fn shuffle_off_keeps_canonical_current() {
        for seed in 1..=30u64 {
            let mut q = QueueCore::new();
            q.replace(vec![t("a"), t("b"), t("c"), t("d"), t("e")], 0);
            q.set_seed(seed);
            q.set_shuffle(true);
            // 播放两首（移动 cursor）。
            let _ = q.skip_next();
            let _ = q.skip_next();
            let canonical_before = q.current_idx().unwrap();
            let playing = q.current().cloned().unwrap();
            q.set_shuffle(false);
            assert_eq!(
                q.current(),
                Some(&playing),
                "seed {seed}: 关闭 shuffle 后当前曲目不得改变（应保留 canonical 曲目而非 cursor 数值）"
            );
            assert_eq!(q.current_idx(), Some(canonical_before), "seed {seed}");
        }
    }

    /// P1：shuffle 与 LoopStatus 正交——None 模式下随机序列到头即停，不隐含列表循环。
    #[test]
    fn shuffle_none_loop_stops_at_end_of_cycle() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 0);
        q.set_seed(7);
        q.set_shuffle(true);
        // 访问完整个随机周期（3 首）后，None 循环 → 停止。
        let _ = q.skip_next();
        let _ = q.skip_next();
        assert_eq!(
            q.skip_next(),
            None,
            "shuffle + None 循环到随机序列末尾应停止（旧行为隐含列表循环）"
        );
        assert!(!q.can_go_next());
    }

    /// P1：shuffle + List 循环 → 随机序列末尾回绕到周期第一首。
    #[test]
    fn shuffle_with_list_loop_wraps() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 0);
        q.set_seed(7);
        q.set_loop_mode(LoopMode::List);
        q.set_shuffle(true);
        let first = q.current().cloned().unwrap();
        let _ = q.skip_next();
        let _ = q.skip_next();
        assert_eq!(q.skip_next(), Some(first), "List + shuffle 周期末回绕");
        assert!(q.can_go_next());
    }

    #[test]
    fn revision_bumps_on_structure_changes() {
        let mut q = QueueCore::new();
        assert_eq!(q.revision(), 0);
        q.append(vec![t("a"), t("b")]);
        assert_eq!(q.revision(), 1);
        q.set_current(0);
        assert_eq!(q.revision(), 2);
        q.remove(1);
        assert_eq!(q.revision(), 3);
        q.set_loop_mode(LoopMode::Track);
        assert_eq!(q.revision(), 4);
        q.set_shuffle(true);
        assert_eq!(q.revision(), 5);
        // 快照与保存不递增（结构未变）。
        let r = q.revision();
        let _ = q.snapshot();
        let s = q.save_state();
        assert_eq!(q.revision(), r);
        // restore 改变结构 → 递增。
        q.restore_state(s);
        assert_eq!(q.revision(), r + 1);
    }

    #[test]
    fn replace_bumps_revision() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b")], 0);
        assert_eq!(q.revision(), 1);
        q.replace(vec![], 0);
        assert_eq!(q.revision(), 2, "空队列 replace 也改变结构");
    }

    #[test]
    fn summary_is_o1_and_reflects_state() {
        let mut q = QueueCore::new();
        q.append(vec![t("a"), t("b"), t("c")]);
        q.set_current(1);
        q.set_loop_mode(LoopMode::List);
        q.set_shuffle(true);
        let s = q.summary();
        assert_eq!(s.revision, q.revision());
        assert_eq!(s.len, 3);
        assert_eq!(s.current, Some(1));
        assert_eq!(s.loop_mode, LoopMode::List);
        assert!(s.shuffle);
    }
}
