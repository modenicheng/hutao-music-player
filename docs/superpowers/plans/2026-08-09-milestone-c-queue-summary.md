# QueueSummary State Protocol Implementation Plan（里程碑 C）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 拆掉 `DaemonState` 内嵌的完整队列：`queue` 字段改为 O(1) 的 `QueueSummary { revision, len, current, loop_mode, shuffle }`；完整队列内容走独立 `queue_rx` watch 通道（仅结构变化时快照）；position tick 的 publish 不再 O(n) 克隆队列（审计 P1 #5；roadmap 里程碑 C）。

**Architecture:** hmp-core `QueueCore` 加 `revision: u64`（每次变更 +1）与 `summary() -> QueueSummary`（O(1)）；`DaemonState.queue` 类型改为 `QueueSummary`；engine 增加 `queue_tx/queue_rx: watch<QueueSnapshot>`（EngineHandle 暴露），`publish()` 仅在 revision 变化时发送完整快照；server 的 `Request::Queue`/`QueueList` 改读 `queue_rx`。

**Tech Stack:** Rust workspace（hmp-core / hmp-daemon / hmp-cli）；tokio watch；serde（QueueSummary 需 Serialize/Deserialize，跨进程）。

## Global Constraints

- 保持 `cargo test --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 全绿。
- `QueueSummary` 必须 `#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]`（DaemonState 跨进程）。
- `QueueSnapshot`/`QueueState`/`QueuePage`/`Response::Queue`/`QueueList` 保持不变（协议不破坏）；`DaemonState.queue` 类型变更由编译器强制。
- `QueueCore` 所有**结构变更**方法（replace/append/insert_after_current/remove/clear/clear_pending/skip_next/advance_on_eos/prev_track/set_current/set_loop_mode/set_shuffle）revision+1；`snapshot()`/`save_state()`/`restore_state()` 不改 revision（restore 会改状态——由调用方（engine 失败回滚）在 restore 后显式 bump 或 restore 内部 bump：**决策：restore_state 内部 bump**，因为状态确实变了）。
- 每个任务独立 commit（`feat(core,…):` 前缀 + 中文要点）。

---

### Task 1: QueueCore.revision + QueueSummary（TDD）

**Files:**
- Modify: `crates/hmp-core/src/queue.rs`
- Test: `crates/hmp-core/src/queue.rs` tests

**Interfaces:**
- Produces（Task 2 依赖）:
  ```rust
  /// 队列摘要（O(1)，随 DaemonState 发布；完整内容走 QueueList/queue watch）。
  #[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
  pub struct QueueSummary {
      /// 队列结构版本（每次变更 +1；position tick 不递增）。
      pub revision: u64,
      /// 队列总曲目数。
      pub len: usize,
      /// 当前曲目位置（规范下标）。
      pub current: Option<usize>,
      pub loop_mode: LoopMode,
      pub shuffle: bool,
  }

  impl QueueCore {
      /// 当前结构版本（变更方法自动递增）。
      pub fn revision(&self) -> u64
      /// O(1) 摘要（不克隆 tracks）。
      pub fn summary(&self) -> QueueSummary
  }
  ```

- [ ] **Step 1: 写测试（先行）**

`crates/hmp-core/src/queue.rs` tests 模块追加：

```rust
    #[test]
    fn revision_bumps_on_structure_changes() {
        let mut q = QueueCore::new();
        assert_eq!(q.revision(), 0);
        q.append(vec![TrackId::new("a"), TrackId::new("b")]);
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
    fn summary_is_o1_and_reflects_state() {
        let mut q = QueueCore::new();
        q.append(vec![TrackId::new("a"), TrackId::new("b"), TrackId::new("c")]);
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
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-core revision_bumps_on_structure_changes`
Expected: FAIL（编译失败：`revision`/`summary` 不存在）。

- [ ] **Step 3: 实现**

`crates/hmp-core/src/queue.rs`：

`QueueCore` 结构加字段 `revision: u64,`（`new()` 初始化 0）。

每个变更方法末尾加 `self.revision += 1;`（方法清单：`replace`、`append`、`insert_after_current`（仅在返回 Some 时——方法体在 `if let Some(at)` 分支内末尾递增；注意方法可能返回 None 不改变结构时不递增）、`remove`（返回 true 时递增）、`clear`、`clear_pending`（pending 清理改变结构——若当前实现无变化则不加，按实现判断）、`set_current`、`set_loop_mode`、`set_shuffle`、`skip_next`（返回 Some 时）、`advance_on_eos`（返回 Some 时）、`prev_track`（返回 Some 时）、`restore_state`）。

`QueueSummary` 结构定义（`LoopMode` 从 `crate::player::LoopMode` 导入，文件顶部已有 use——确认后沿用）。

```rust
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
```

注意 `current_idx()` 已存在（233 行）——确认其返回 `Option<usize>` 语义（has_current 时 Some）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p hmp-core`
Expected: 全绿（含现有队列测试——revision 递增不影响既有断言）。

- [ ] **Step 5: Commit**

```bash
git add crates/hmp-core/src/queue.rs
git commit -m "feat(core): QueueCore.revision + QueueSummary - O(1) queue digest for DaemonState"
```

---

### Task 2: DaemonState.queue → QueueSummary + engine queue watch（TDD）

**Files:**
- Modify: `crates/hmp-core/src/ipc.rs`（`DaemonState.queue` 类型；`use`）
- Modify: `crates/hmp-daemon/src/engine.rs`（`queue_tx/queue_rx`、`last_queue_rev`、`publish()` 拆分、`EngineHandle` 字段、`start_with_*` 初始化）
- Modify: `crates/hmp-daemon/src/server.rs`（`Request::Queue`/`QueueList` 读 `queue_rx`）
- Test: `crates/hmp-daemon/src/engine.rs` tests（约 20 处 `queue.tracks` 断言适配）+ `crates/hmp-core/src/ipc.rs`（daemon_state_roundtrips 适配）

**Interfaces:**
- Consumes: Task 1 的 `QueueCore::revision()`/`summary()`、`QueueSummary`。
- Produces（Task 3 依赖）:
  - `DaemonState { …, pub queue: QueueSummary, … }`
  - `EngineHandle.queue_rx: watch::Receiver<QueueSnapshot>`（完整队列；仅结构变化时更新）
  - server：`Request::Queue` → `Response::Queue(queue_rx.borrow().clone())`；`QueueList` 分页从 `queue_rx.borrow()` 切页。

- [ ] **Step 1: hmp-core 类型替换**

`crates/hmp-core/src/ipc.rs`：`use crate::queue::QueueSnapshot;` → `use crate::queue::{QueueSnapshot, QueueSummary};`；`DaemonState` 的 `pub queue: QueueSnapshot,` → `pub queue: QueueSummary,`。`daemon_state_roundtrips` 测试的 `queue: crate::queue::QueueSnapshot::default()` → `QueueSummary::default()`。

Run: `cargo test -p hmp-core`
Expected: 编译通过、全绿（hmp-core 内部无其他消费点；daemon 编译错误留给 Step 2）。

- [ ] **Step 2: engine 加 queue watch + publish 拆分**

`crates/hmp-daemon/src/engine.rs`：

`PlaybackEngine` 结构加字段：

```rust
    /// 完整队列快照（仅结构变化时发送；position tick 不触发——O(1) publish）。
    queue_tx: watch::Sender<QueueSnapshot>,
    /// 上次发布的队列版本（避免重复发送）。
    last_queue_rev: u64,
```

`EngineHandle` 结构加字段：

```rust
    /// 完整队列（结构变更时更新；消费方：server 的 Queue/QueueList）。
    pub queue_rx: watch::Receiver<hmp_core::QueueSnapshot>,
```

`start_with_options`（或 `start_with_library`——以当前代码为准）中：

```rust
        let (queue_tx, queue_rx) = watch::channel(hmp_core::QueueSnapshot::default());
```

初始化 `queue_tx, last_queue_rev: 0`；`EngineHandle` 构造加 `queue_rx`。

`publish()` 改为：

```rust
    /// 发布复合状态（playback 来自驱动 watch，queue 摘要 O(1)）。
    /// 完整队列快照仅在结构变化时发送到 `queue_tx`（position tick 不克隆）。
    /// 同时把精确的播放能力发布到 `caps_tx`（MPRIS 消费，Finding 9）。
    fn publish(&self) {
        let caps = PlaybackCapabilities {
            can_go_next: self.queue.can_go_next(),
            can_go_previous: self.queue.can_go_previous(),
        };
        let state = DaemonState {
            playback: self.state_rx.borrow().clone(),
            queue: self.queue.summary(),
            caps,
            seq: self.seq,
            last_error: self.last_error.clone(),
            phase: self.phase,
        };
        let _ = self.state_tx.send(state);
        if self.last_queue_rev != self.queue.revision() {
            self.last_queue_rev = self.queue.revision();
            let _ = self.queue_tx.send(self.queue.snapshot());
        }
        let _ = self.caps_tx.send(caps);
    }
```

注意：`QueueSnapshot` 已在 engine.rs use（`use hmp_core::{…}` 里可能没有——补 `use hmp_core::QueueSnapshot;` 或全限定）。

- [ ] **Step 3: server 改读 queue_rx**

`crates/hmp-daemon/src/server.rs`（约 140-152 行）：

```rust
        Ok(Request::Queue) => {
            let resp = Response::Queue(handle.queue_rx.borrow().clone());
            write_frame(wr, &resp).await?;
        }
        Ok(Request::QueueList { offset, limit }) => {
            // 纯 ID 分页（server 无媒体库引用；标题投影在 CLI 侧）。
            let snap = handle.queue_rx.borrow().clone();
            let total = snap.tracks.len();
            let items = snap
                .tracks
                .iter()
                .enumerate()
                .skip(offset)
                .take(limit)
                .map(|(i, id)| QueueEntry {
                    track_id: id.clone(),
                    is_current: Some(i) == snap.current,
                })
                .collect();
            let page = QueuePage { total, offset, items };
            write_frame(wr, &Response::QueueList(page)).await?;
        }
```

（原有代码结构保持；只把 `handle.state_rx.borrow().queue.clone()` 改为 `handle.queue_rx.borrow().clone()`，`snap.tracks`/`snap.current` 字段不变。）

- [ ] **Step 4: 适配 engine 测试（约 20 处）**

`crates/hmp-daemon/src/engine.rs` tests：所有 `handle.state_rx.borrow().queue.tracks` / `state.queue.tracks` 断言改为读完整队列（`handle.queue_rx.borrow().tracks`）；`state.queue.current` 改为 `state.queue.current`（summary 的 current 字段同名 ✓ 但类型变了——`queue.current` 在 QueueSummary 里是 `Option<usize>` ✓ 与 QueueSnapshot 相同，直接兼容）；`queue.tracks.len()` 断言改 `queue_rx.borrow().tracks.len()` 或 `queue.len`（summary）。

逐个检查以下位置的断言语义（约 15-20 处，行号以当前文件为准）：
- `assert_eq!(handle.state_rx.borrow().queue.tracks.len(), 3)` → `handle.queue_rx.borrow().tracks.len()`
- `state.queue.tracks[0].as_ref() == "local:/tmp/x.mp3"` → `handle.queue_rx.borrow().tracks[0]`
- `state.queue.tracks.is_empty()` → `handle.queue_rx.borrow().tracks.is_empty()`
- `assert_eq!(st.queue.tracks, vec![…])` → `st.queue_rx.borrow().tracks`（注意 borrow 作用域）
- `assert!(s.queue.tracks.is_empty())`（事件流里的 state）——订阅 `state_rx.changed()` 的测试，读 `s.queue.tracks` 需改 `s.queue.len` 或加 queue_rx 订阅
- `failed_load_rolls_back_to_previous_track` 等测试的 `queue.current == Some(0)` —— QueueSummary.current 同字段名，无需改

提示：`queue_rx.borrow()` 返回 `QueueSnapshot`（含 tracks/current）——大多数断言 1:1 替换字段前缀即可。事件流测试（`Event::StateChanged(s)`）中若需完整队列，测试侧改为订阅 `handle.queue_rx`（`queue_rx.changed()` 等待初始值注意 watch 语义：先 borrow 再 changed）。

- [ ] **Step 5: 全量验证**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

- [ ] **Step 6: Commit**

```bash
git add crates/hmp-core/src/ipc.rs crates/hmp-daemon/src/engine.rs crates/hmp-daemon/src/server.rs
git commit -m "feat(core,daemon): DaemonState.queue -> QueueSummary; full queue via dedicated watch (O(1) publish)"
```

---

### Task 3: CLI 消费适配 + 大队列体积测试

**Files:**
- Modify: `crates/hmp-cli/src/commands.rs`（status 显示、测试构造）
- Test: `crates/hmp-daemon/src/engine.rs` tests（新增体积测试）+ `crates/hmp-core/src/ipc.rs`（QueueSummary 往返）

**Interfaces:**
- Consumes: Task 2 的 `DaemonState.queue: QueueSummary`、`EngineHandle.queue_rx`。

- [ ] **Step 1: CLI status 适配**

`crates/hmp-cli/src/commands.rs:44`：

```rust
    s.push_str(&format!("队列: {} 首\n", st.queue.len));
```

`commands.rs:420/486`（测试构造 DaemonState）：`queue: Default::default(),` 不变（Default 派生）——确认编译即可。

- [ ] **Step 2: 新增体积测试（防 IPC 超帧回归）**

`crates/hmp-daemon/src/engine.rs` tests 追加：

```rust
    #[tokio::test]
    async fn large_queue_publish_stays_small() {
        // 万级队列：DaemonState 发布体积必须远小于 MAX_FRAME（队列内容不走状态帧）。
        let (driver, _, _) = FakeDriver::new();
        let ids: Vec<TrackId> = (0..10_000)
            .map(|i| TrackId::new(format!("mid-{i}")))
            .collect();
        let resolver = FakeResolver::new(vec![ids.clone()]);
        let handle = PlaybackEngine::start(driver.clone(), resolver, || true);
        handle.cmd(Request::Play(PlayRequest::Track(ids[0].clone()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let st = handle.state_rx.borrow().clone();
        assert_eq!(st.queue.len, 10_000);
        let frame = hmp_core::encode_frame(&st).unwrap();
        assert!(
            frame.len() < hmp_core::MAX_FRAME / 4,
            "万级队列状态帧应保持小体积，实际 {} 字节",
            frame.len()
        );
        // 完整队列仍可经 queue_rx 取到。
        assert_eq!(handle.queue_rx.borrow().tracks.len(), 10_000);
    }
```

注意：`FakeResolver::new(vec![ids.clone()])` 构造后 `Play` 一次消耗列表；`Request::Play` 的 Track 源解析只取第一个 stub——检查 `play_source` 对 Track 源的行为（resolve_source_ids 弹出列表全部 10000 个 → `load_and_play(ids[0])` + `queue.replace(ids, 0)`）✓。

- [ ] **Step 3: QueueSummary 序列化往返测试**

`crates/hmp-core/src/ipc.rs` tests 追加：

```rust
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
```

- [ ] **Step 4: 全量 + e2e**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e && cargo test -p hmp-cli --test daemon_cli -- --ignored`
Expected: 全绿。

- [ ] **Step 5: Commit**

```bash
git add crates/hmp-cli/src/commands.rs crates/hmp-daemon/src/engine.rs crates/hmp-core/src/ipc.rs
git commit -m "feat(cli,core): QueueSummary consumers - status uses len, 10k-queue publish size test"
```

---

### Task 4: 收尾验证

- [ ] **Step 1: 全量验证**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

- [ ] **Step 2: 核对覆盖**

对照审计 P1 #5：DaemonState 不再内嵌完整队列（✓）；position tick 不克隆队列（publish 只在 revision 变化时 snapshot ✓）；IPC 帧大小（万级队列测试 ✓）；`Response::Queue`/`QueueList` 功能等价（server 读 queue_rx ✓）。`QueueSnapshot` 协议类型保留（不破坏客户端）。

- [ ] **Step 3: 报告**

报告：changed files、每任务验证、`cargo test --workspace` 总通过数、clippy/fmt 状态、未完成项。不要 git push（父会话统一推送并更新 roadmap）。
