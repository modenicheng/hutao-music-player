# Playback Transaction (load generation + explicit session) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让"换曲事务"覆盖真正播放器装载：引入装载代际（load generation）彻底替代 500ms 滞后事件猜测窗口；driver 明确 ACK 新曲后才提交（队列/会话/旧媒体释放）；播放历史改用显式 event id，listened_ms 记录旧曲真实位置。

**Architecture:** 三层改动——(1) hmp-core `PlaybackState` 加 `gen` 字段 + hmp-player-gst `LoadRequest` 加 `gen`、离散事件携带 gen；(2) engine 用 `current_gen` 过滤旧代事件（删除 `loaded_at` 窗口），装载失败尽力回滚到上一曲；(3) 播放会话从 `Option<i64>` 升级为 `PlaybackSession { track_id, event_id }`，`record_play_start` 返回 event id，`record_play_end(event_id, …)` 精确闭合，换曲路径在装载**前**捕获旧 position 作为 listened_ms。

**Tech Stack:** Rust workspace（hmp-core / hmp-player-gst / hmp-storage / hmp-daemon）；tokio watch/broadcast；rusqlite；TDD（每任务先写失败测试）。

## Global Constraints

- 保持 `cargo test --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 全绿（任务 5 结束后跑全量）。
- `PlaybackState` 是跨进程序列化类型：新字段必须 `#[serde(default)]`（老客户端兼容）。
- 禁止改动 `PlayerCommand` 枚举变体与 `Request` 协议（本次不扩协议；generation 是内部机制）。
- 不引入新依赖。
- 每个任务独立 commit；commit 信息用现有风格（`feat:`/`fix:` 前缀 + 中文要点）。
- 队列事务语义保持现有约定：装载成功才提交队列变更/关闭旧会话；失败回滚队列、旧曲继续。
- 中文注释风格与现有代码一致。

---

### Task 1: PlaybackState.gen + LoadRequest.gen + 装载时丢弃旧总线事件

**Files:**
- Modify: `crates/hmp-core/src/player.rs`（PlaybackState 结构）
- Modify: `crates/hmp-player-gst/src/core.rs`（LoadRequest、drive()）
- Test: `crates/hmp-player-gst/src/core.rs` tests 模块

**Interfaces:**
- Produces: `PlaybackState { …, pub gen: u64 }`（`#[serde(default)]`）；`hmp_player_gst::LoadRequest { track, uri, quality, gen: u64 }`；`PlayerCore::load`/`GstDriver::load` 透传 gen；drive() 在装载处理时置 `state.gen = req.gen` 并 **drain** `bus_rx` 队列（丢弃装载前已入队的旧曲事件）。

- [ ] **Step 1: hmp-core 加字段**

在 `crates/hmp-core/src/player.rs` 的 `PlaybackState` 中 `actual_quality` 之后加：

```rust
    /// 实际播放音质（本次解析选定档位；媒体库重构 B3）。
    #[serde(default)]
    pub actual_quality: Option<AudioQuality>,
    /// 装载代际：每次 driver 装载递增（engine 分配），事件/状态过滤用。
    #[serde(default)]
    pub gen: u64,
```

`Default` impl 里 `actual_quality: None,` 之后加 `gen: 0,`。`playback_state_default_is_empty` 测试加断言 `assert_eq!(s.gen, 0);`。

- [ ] **Step 2: 跑 hmp-core 测试确认通过**

Run: `cargo test -p hmp-core`
Expected: 全绿（新断言通过；序列化测试不受影响，serde default 兜底）。

- [ ] **Step 3: hmp-player-gst LoadRequest 加 gen**

`crates/hmp-player-gst/src/core.rs`：

```rust
/// 加载请求：曲目元数据 + 播放 URI。
#[derive(Clone, Debug)]
pub struct LoadRequest {
    /// 曲目元数据（供状态发布与上层展示）。
    pub track: Track,
    /// 播放 URI（http/https/本地文件）。
    pub uri: String,
    /// 请求音质（记录用）。
    pub quality: hmp_core::AudioQuality,
    /// 装载代际（engine 分配；事件与状态过滤用）。
    pub gen: u64,
}
```

- [ ] **Step 4: drive() 记录代际 + 丢弃旧总线事件**

`drive()` 函数签名后加局部变量：

```rust
    let mut state = PlaybackState::default();
    let mut pending_error: Option<String> = None;
    let mut loaded_gen: u64 = 0;
```

`LoadCommand::Load(req)` 分支改为：

```rust
                    LoadCommand::Load(req) => {
                        // 加载即播放（docs/PROJECT.md §8.2：设置 URI → Loading → Playing）
                        loaded_gen = req.gen;
                        state.status = PlaybackStatus::Loading;
                        state.current = Some(req.track);
                        state.actual_quality = Some(req.quality);
                        state.gen = req.gen;
                        state.position = Duration::ZERO;
                        state.duration = None;
                        state.buffering = None;
                        // 丢弃装载前已入队的旧曲总线事件（位置/时长/状态回调可能滞后，
                        // 代际隔离的兜底：避免旧 Position/Duration 污染新曲状态）。
                        while bus_rx.try_recv().is_ok() {}
                        player.set_uri(Some(req.uri.as_str()));
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::TrackChanged);
                        player.play();
                        state.status = PlaybackStatus::Playing;
                        let _ = state_tx.send(state.clone());
                    }
```

- [ ] **Step 5: 修 core.rs 现有测试的 LoadRequest 构造**

`load_req()` helper 加 `gen: 0`：

```rust
    fn load_req(track: Track, uri: &str) -> LoadRequest {
        LoadRequest {
            track,
            uri: uri.to_owned(),
            quality: AudioQuality::Mp3_128,
            gen: 0,
        }
    }
```

- [ ] **Step 6: 写 gen 传播测试（失败先行）**

`crates/hmp-player-gst/src/core.rs` tests 模块追加：

```rust
    #[tokio::test]
    async fn load_sets_gen_on_state() {
        let core = PlayerCore::new_with_sink(Some("fakeaudiosink")).expect("init");
        let rx = core.subscribe_state();
        let mut req = load_req(sample_track(), "file:///nonexistent.aiff");
        req.gen = 42;
        core.load(req);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(rx.borrow().gen, 42);
        core.shutdown();
    }
```

- [ ] **Step 7: 跑测试确认先失败**

Run: `cargo test -p hmp-player-gst load_sets_gen_on_state`
Expected: FAIL（`assert_eq!` 失败：state.gen 为 0 而非 42）——证明测试有效。

- [ ] **Step 8: 跑 hmp-player-gst 全测试确认通过**

Run: `cargo test -p hmp-player-gst`
Expected: 全绿（含已有 4 个状态测试；`load_publishes_track_and_loading` 等不受影响）。

- [ ] **Step 9: Commit**

```bash
git add crates/hmp-core/src/player.rs crates/hmp-player-gst/src/core.rs
git commit -m "feat(core,gst): load generation - PlaybackState.gen + LoadRequest.gen, drain stale bus events on load"
```

---

### Task 2: PlayerEvent 携带装载代际

**Files:**
- Modify: `crates/hmp-player-gst/src/events.rs`
- Modify: `crates/hmp-player-gst/src/core.rs`（Eos/Error 发布点）
- Test: `crates/hmp-player-gst/src/core.rs` tests 模块

**Interfaces:**
- Consumes: Task 1 的 `loaded_gen: u64`（drive 内局部变量）。
- Produces: `PlayerEvent::PlaybackEnded { gen: u64 }`、`PlayerEvent::Error { gen: u64, error: HmpError }`。消费方（engine.rs run() 两处匹配 + 测试 emit 调用点、hmp-daemon/tests/e2e.rs 一处匹配）必须同步更新——编译器强制。

- [ ] **Step 1: events.rs 改枚举**

`crates/hmp-player-gst/src/events.rs`：

```rust
/// 播放器离散事件。
#[derive(Clone, Debug)]
pub enum PlayerEvent {
    /// 已加载新曲目（URI 已设置）。
    TrackChanged,
    /// 播放到结尾（EOS）；携带装载代际（engine 过滤旧代）。
    PlaybackEnded { gen: u64 },
    /// 播放出错；携带装载代际。
    Error { gen: u64, error: HmpError },
    /// 缓冲进度变化（0.0..=1.0，None=结束缓冲）。
    BufferingChanged(Option<f64>),
}
```

- [ ] **Step 2: core.rs 发布点带 gen**

`drive()` 中 `BusEvent::Error(msg)` 分支：

```rust
                    BusEvent::Error(msg) => {
                        pending_error = Some(msg.clone());
                        state.status = PlaybackStatus::Error;
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::Error {
                            gen: loaded_gen,
                            error: HmpError::Playback(msg),
                        });
                    }
```

`BusEvent::Eos` 分支：

```rust
                    BusEvent::Eos => {
                        state.status = PlaybackStatus::Ended;
                        state.position = state.duration.unwrap_or(state.position);
                        let _ = state_tx.send(state.clone());
                        let _ = events_tx.send(PlayerEvent::PlaybackEnded { gen: loaded_gen });
                    }
```

- [ ] **Step 3: 修 engine.rs 与 e2e.rs 的编译错误（匹配更新，行为不变）**

`crates/hmp-daemon/src/engine.rs` run() 两处（247 行附近、260 行附近）：

```rust
                        Ok(PlayerEvent::PlaybackEnded { gen: _ }) => {
```

```rust
                        Ok(PlayerEvent::Error { .. }) => {
```

engine.rs 测试内所有 `driver.emit(PlayerEvent::PlaybackEnded)` 调用点（约 8 处）改为 `driver.emit(PlayerEvent::PlaybackEnded { gen: 0 })`；`hmp-daemon/tests/e2e.rs:476` 的 `Ok(hmp_player_gst::PlayerEvent::PlaybackEnded)` 改为 `Ok(hmp_player_gst::PlayerEvent::PlaybackEnded { .. })`。

- [ ] **Step 4: 跑测试确认编译+全绿**

Run: `cargo test --workspace`
Expected: 全绿（此任务只改形状不改行为）。

- [ ] **Step 5: 写错误事件 gen 测试（失败先行）**

`crates/hmp-player-gst/src/core.rs` tests 模块追加（bad uri 可靠触发 Error）：

```rust
    #[tokio::test]
    async fn error_event_carries_loaded_gen() {
        let core = PlayerCore::new_with_sink(Some("fakeaudiosink")).expect("init");
        let mut ev = core.subscribe_events();
        let mut req = load_req(sample_track(), "file:///definitely/missing/file.aiff");
        req.gen = 7;
        core.load(req);
        core.play();
        let mut got: Option<u64> = None;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Ok(e) = ev.try_recv() {
                if let PlayerEvent::Error { gen, .. } = e {
                    got = Some(gen);
                }
            }
            if got.is_some() {
                break;
            }
        }
        assert_eq!(got, Some(7), "Error 事件必须携带装载代际 7");
        core.shutdown();
    }
```

- [ ] **Step 6: 跑测试确认先失败再通过**

Run: `cargo test -p hmp-player-gst error_event_carries_loaded_gen`
Expected: 先 FAIL（Step 2 未做时 gen 不存在——实际编译失败即"失败"）；Step 2 后 PASS。若 Step 2 已先行，此测试直接 PASS——顺序以实际为准，测试必须真实断言 `gen == 7`。

- [ ] **Step 7: Commit**

```bash
git add crates/hmp-player-gst/src/events.rs crates/hmp-player-gst/src/core.rs crates/hmp-daemon/src/engine.rs crates/hmp-daemon/tests/e2e.rs
git commit -m "feat(gst): PlayerEvent carries load generation (PlaybackEnded/Error); update consumers"
```

---

### Task 3: engine 用 current_gen 过滤旧代事件，删除 500ms 窗口

**Files:**
- Modify: `crates/hmp-daemon/src/engine.rs`
- Test: `crates/hmp-daemon/src/engine.rs` tests 模块（loading_window / stale_eos 测试重写）

**Interfaces:**
- Consumes: Task 2 的 `PlayerEvent::PlaybackEnded { gen }` / `PlayerEvent::Error { gen, .. }`。
- Produces: `PlaybackEngine.current_gen: u64`；`load_and_play` 每次装载 `self.current_gen += 1` 并写入 `LoadRequest.gen`；run() 事件分支按 `gen != self.current_gen` 忽略。删除字段 `loaded_at` 与 `EnginePhase::Loading` 窗口判断。

- [ ] **Step 1: 加字段、删窗口**

`PlaybackEngine` 结构：删除 `loaded_at: Option<std::time::Instant>`，加：

```rust
    /// 装载代际（每次 load_and_play 递增；旧代事件过滤，spec §7 换代机制）。
    current_gen: u64,
```

`start_with_library` 初始化：删 `loaded_at: None,`，加 `current_gen: 0,`。

- [ ] **Step 2: run() 事件分支按代际过滤**

`Ok(PlayerEvent::PlaybackEnded { gen })` 分支（替换原窗口判断）：

```rust
                        Ok(PlayerEvent::PlaybackEnded { gen }) => {
                            // 代际过滤（spec §7）：旧代 EOS 属已换下的曲目 → 忽略，
                            // 不触发换曲。同代 EOS 是真实曲尾（短曲立即结束也要续播）。
                            if gen != self.current_gen {
                                tracing::debug!(gen, current = self.current_gen, "忽略旧代 EOS");
                            } else {
                                self.on_ended().await;
                            }
                        }
                        Ok(PlayerEvent::Error { gen, .. }) => {
                            // 旧代错误事件属已换下的曲目 → 忽略（装载结果由 load_and_play 决定）。
                            if gen != self.current_gen {
                                tracing::debug!(gen, current = self.current_gen, "忽略旧代错误事件");
                            } else {
                                self.publish();
                            }
                        }
```

- [ ] **Step 3: load_and_play 分配代际**

`load_and_play` 中 `self.driver.load(...)` 之前：

```rust
                self.current_gen += 1;
                let gen = self.current_gen;
                self.driver.load(hmp_player_gst::LoadRequest {
                    track: res.track.clone(),
                    uri,
                    quality,
                    gen,
                });
```

删除成功路径中的 `self.loaded_at = Some(std::time::Instant::now());` 行（窗口机制已废）。

- [ ] **Step 4: 重写窗口测试为代际测试（失败先行）**

`crates/hmp-daemon/src/engine.rs` tests 模块中，删除 `loading_window_ignores_stale_eos`（1478 行附近）与 `stale_eos_after_load_window_is_ignored`（1511 行附近），替换为：

```rust
    #[tokio::test]
    async fn stale_gen_eos_is_ignored() {
        // 旧代 EOS（已换下曲目的迟到事件）不得触发换曲——不再依赖 500ms 窗口。
        let (driver, _, events) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![track_a(), track_b()]]);
        let handle = PlaybackEngine::start(driver.clone(), resolver, || true);
        handle.cmd(Request::Play(PlayRequest::Track(track_a()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // 手动换到 b（gen 递增）。
        handle.cmd(Request::Play(PlayRequest::Track(track_b()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cur_before = handle.state_rx.borrow().current.clone();
        // 旧代 EOS 到达：任何时刻都应忽略（不换曲、不关会话）。
        driver.emit(PlayerEvent::PlaybackEnded { gen: 1 });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(handle.state_rx.borrow().current, cur_before);
    }

    #[tokio::test]
    async fn same_gen_eos_advances_immediately() {
        // 同代 EOS = 真实曲尾：装载完成后立即到达也须续播（旧 500ms 窗口会丢短曲）。
        let (driver, _, events) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![track_a(), track_b()]]);
        let handle = PlaybackEngine::start(driver.clone(), resolver, || true);
        handle.cmd(Request::Play(PlayRequest::Track(track_a()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let gen = driver.loads.lock().unwrap().len() as u64; // 首载 gen=1
        driver.emit(PlayerEvent::PlaybackEnded { gen });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let s = handle.state_rx.borrow();
        assert_eq!(s.current.as_ref().unwrap().id, track_b(), "同代 EOS 应立即续播");
    }
```

注意：`driver.loads.lock().unwrap().len() as u64` 是首载后的代际值（load_and_play 先 `current_gen += 1` 再 load，故首载 gen == 1）。若现有 helper 名不同（`track_a()` 等），沿用 tests 模块已有 helper（现有测试如 `advance_on_eos` 附近应有类似构造，仿照它们的建队方式）。

- [ ] **Step 5: 适配受影响测试**

搜索 tests 模块中所有 `loaded_at`、`emit(PlayerEvent::PlaybackEnded)` 调用点：emit 调用点保留（gen 参数按场景：模拟"当前曲 EOS"的测试用与当前代一致的值——若测试在单次装载后立即 emit，用 `1`；若无法确定，用 `0` 会让 `gen != current_gen` 而忽略——因此需要逐个按语义改：单曲装载后 emit 的测试用当前代（先 `handle.cmd(Play(...))` 后 emit 的场景，代际为 1）。`phase_transitions_on_load_failure`（1541 行附近）不受影响，保留。

- [ ] **Step 6: 跑 engine 测试**

Run: `cargo test -p hmp-daemon --lib`
Expected: 全绿。若 `stale_gen_eos_is_ignored` 需要等待 EOS 在窗口外被忽略——旧实现下该测试会 FAIL（旧代码无 gen 概念会触发换曲），先确认失败再实现（TDD 顺序：Step 4 写在 Step 2/3 之前或按实际）。

- [ ] **Step 7: 全量 + Commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: 全绿。

```bash
git add crates/hmp-daemon/src/engine.rs
git commit -m "feat(daemon): load generation event filtering - remove 500ms stale-EOS window"
```

---

### Task 4: 装载提交顺序——ACK 后才替换 active_media，失败尽力回滚装载

**Files:**
- Modify: `crates/hmp-daemon/src/engine.rs`（load_and_play、新增 rollback_load、AppliedLoad、load_timeout 注入）
- Modify: `crates/hmp-daemon/src/player.rs`（EngineError 已有 Timeout；不动）
- Test: `crates/hmp-daemon/src/engine.rs` tests 模块

**Interfaces:**
- Consumes: Task 3 的 `current_gen`；现有 `wait_current_applied`（返回 `Result<(), EngineError>`）。
- Produces:
  - `struct AppliedLoad { track: Track, uri: String, quality: AudioQuality, gen: u64 }`（engine.rs 私有）
  - `PlaybackEngine.last_load: Option<AppliedLoad>`、`PlaybackEngine.load_timeout: Duration`
  - `PlaybackEngine::start_with_options(driver, resolver, credential_ok, library, load_timeout: Duration) -> EngineHandle`；`start_with_library` 委托它传 `Duration::from_secs(5)`。
  - `async fn rollback_load(&mut self, prev: AppliedLoad, position: Duration)`：重载上一曲（沿用原 gen）→ `wait_current_applied` 确认 → seek(旧位置) → play；未确认仅 warn。
  - `FakeDriver::set_fail_load(bool)`（测试脚手架）：置位后下一次 `load` 不更新 current（wait 超时）。

- [ ] **Step 1: 写失败测试（TDD 先行）**

tests 模块追加（使用现有 helper；`start_with_options` 尚未存在——本测试同时驱动其诞生）：

```rust
    #[tokio::test]
    async fn failed_load_rolls_back_to_previous_track() {
        // 换曲装载失败：队列回滚、播放器尽力重载上一曲（恢复到旧位置）。
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![track_a(), track_b()]]);
        let handle = PlaybackEngine::start_with_options(
            driver.clone(),
            resolver,
            || true,
            None,
            std::time::Duration::from_millis(300),
        );
        // 先成功播放 a（gen=1）。
        handle.cmd(Request::Play(PlayRequest::Track(track_a()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(driver.loads.lock().unwrap().len(), 1);
        // 让 a 位置前进（回滚后应 seek 回此处）。
        driver.state_tx.send_modify(|s| s.position = std::time::Duration::from_secs(12));
        // 换 b 但装载失败（不更新 current → wait 超时）。
        driver.set_fail_load(true);
        handle.cmd(Request::Play(PlayRequest::Track(track_b()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let loads = driver.loads.lock().unwrap().clone();
        assert_eq!(loads.len(), 2, "失败后应有一次回滚重载");
        assert_eq!(loads[1], "fake://a", "回滚应重载上一曲 a");
        assert!(driver.commands.lock().unwrap().contains(
            &PlayerCommand::Seek(std::time::Duration::from_secs(12))
        ), "回滚应 seek 回旧位置");
        assert_eq!(handle.state_rx.borrow().phase, hmp_core::EnginePhase::Failed);
        // 队列保持 a（play_source 失败路径 restore_state + restore_phase_after_failure）。
        assert_eq!(handle.state_rx.borrow().queue.current, Some(0));
    }

    #[tokio::test]
    async fn first_load_failure_has_no_rollback() {
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![track_a()]]);
        let handle = PlaybackEngine::start_with_options(
            driver.clone(), resolver, || true, None,
            std::time::Duration::from_millis(300),
        );
        driver.set_fail_load(true);
        handle.cmd(Request::Play(PlayRequest::Track(track_a()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert_eq!(driver.loads.lock().unwrap().len(), 1, "首次装载失败无回滚");
        assert_eq!(handle.state_rx.borrow().phase, hmp_core::EnginePhase::Failed);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-daemon --lib failed_load_rolls_back_to_previous_track`
Expected: FAIL（编译失败：`start_with_options` 不存在；或行为失败：无回滚重载）。这是本任务的"失败先行"。

- [ ] **Step 3: FakeDriver 加失败开关**

tests 模块 `FakeDriver`：

```rust
    pub struct FakeDriver {
        pub state_tx: watch::Sender<PlaybackState>,
        pub events_tx: broadcast::Sender<PlayerEvent>,
        pub loads: Mutex<Vec<String>>,
        pub commands: Mutex<Vec<PlayerCommand>>,
        /// 置位后下一次 load 不更新 current（模拟驱动装载失败 → wait 超时）。
        pub fail_next_load: std::sync::atomic::AtomicBool,
    }

    impl FakeDriver {
        pub fn set_fail_load(&self, on: bool) {
            self.fail_next_load
                .store(on, std::sync::atomic::Ordering::SeqCst);
        }
    }
```

`new()` 里初始化 `fail_next_load: std::sync::atomic::AtomicBool::new(false)`；`load` 实现开头：

```rust
        fn load(&self, request: LoadRequest) {
            self.loads.lock().unwrap().push(request.uri.clone());
            if self.fail_next_load.swap(false, std::sync::atomic::Ordering::SeqCst) {
                return; // 失败模拟：current 不更新 → wait_current_applied 超时
            }
            let (track, quality) = (request.track.clone(), request.quality);
            ...
        }
```

- [ ] **Step 4: start_with_options + AppliedLoad + load_timeout**

`PlaybackEngine` 结构加字段：

```rust
    /// 最近一次成功装载（失败回滚用）。
    last_load: Option<AppliedLoad>,
    /// 等待驱动应用装载的超时（测试注入短超时）。
    load_timeout: std::time::Duration,
```

`impl PlaybackEngine`：

```rust
    pub fn start_with_library(
        driver: Arc<dyn PlaybackDriver>,
        resolver: Arc<dyn SourceResolver>,
        credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
        library: Option<std::sync::Arc<std::sync::Mutex<hmp_storage::LibraryDb>>>,
    ) -> EngineHandle {
        Self::start_with_options(
            driver,
            resolver,
            credential_ok,
            library,
            std::time::Duration::from_secs(5),
        )
    }

    /// 带装载超时注入的启动（测试用短超时驱动失败路径）。
    pub fn start_with_options(
        driver: Arc<dyn PlaybackDriver>,
        resolver: Arc<dyn SourceResolver>,
        credential_ok: Arc<dyn Fn() -> bool + Send + Sync>,
        library: Option<std::sync::Arc<std::sync::Mutex<hmp_storage::LibraryDb>>>,
        load_timeout: std::time::Duration,
    ) -> EngineHandle {
        // 现有 start_with_library 函数体整体迁入；结构初始化加：
        //     last_load: None,
        //     load_timeout,
    }
```

文件顶部（`use` 之后）定义：

```rust
/// 最近一次成功装载的完整信息（失败回滚用）。
#[derive(Clone)]
struct AppliedLoad {
    track: hmp_core::Track,
    uri: String,
    quality: hmp_core::AudioQuality,
    gen: u64,
}
```

- [ ] **Step 5: 重写 load_and_play 的提交顺序 + rollback_load**

`load_and_play` 成功分支替换为：

```rust
            Ok(res) => {
                // 捕获上一装载与旧位置（回滚与历史用；此时尚未触碰任何状态）。
                let prev = self.last_load.clone();
                let prev_position = self.state_rx.borrow().position;
                self.current_gen += 1;
                let gen = self.current_gen;
                self.driver.load(hmp_player_gst::LoadRequest {
                    track: res.track.clone(),
                    uri: res.uri.clone(),
                    quality: res.quality,
                    gen,
                });
                self.driver.play();
                // 等待驱动应用装载（真实驱动为异步管道）：完成前发布的复合状态
                // 不得携带旧曲目。超时/通道断开 → 失败路径。
                if let Err(e) = self.wait_current_applied(&res.track.id).await {
                    // 未确认装载：新解密代理此刻释放；旧 active_media 保持。
                    drop(res.media);
                    if let Some(p) = prev {
                        self.rollback_load(p, prev_position).await;
                    }
                    self.last_error = Some(error_info(&e));
                    self.phase = hmp_core::EnginePhase::Failed;
                    self.publish();
                    return Err(e);
                }
                // ACK 成功才提交：替换 active_media（旧代理此刻才释放）、
                // 记录装载（回滚用）、进入播放阶段、开启播放会话。
                self.active_media = res.media;
                self.last_load = Some(AppliedLoad {
                    track: res.track.clone(),
                    uri: res.uri,
                    quality: res.quality,
                    gen,
                });
                self.phase = hmp_core::EnginePhase::Playing;
                self.start_session(&res.track);
                self.publish();
                Ok(())
            }
```

注意 `uri`/`quality` 由 `res.uri.clone()`/`res.quality` 移入 LoadRequest 与 AppliedLoad——原代码有 `let uri = res.uri.clone(); let quality = res.quality;` 局部变量，按需保留或内联，保证不 double-move。

新增方法（`wait_current_applied` 之后）：

```rust
    /// 装载失败后的尽力回滚：重载上一首并恢复到其位置。
    /// 沿用原代际（不干扰 current_gen 的事件过滤）；未确认仅 warn。
    async fn rollback_load(&mut self, prev: AppliedLoad, position: std::time::Duration) {
        let id = prev.track.id.clone();
        self.driver.load(hmp_player_gst::LoadRequest {
            track: prev.track.clone(),
            uri: prev.uri,
            quality: prev.quality,
            gen: prev.gen,
        });
        if self.wait_current_applied(&id).await.is_ok() {
            self.driver.seek(position);
            self.driver.play();
        } else {
            tracing::warn!("回滚装载未确认（旧曲可能无法恢复）");
        }
    }
```

- [ ] **Step 6: 跑测试**

Run: `cargo test -p hmp-daemon --lib failed_load_rolls_back_to_previous_track first_load_failure_has_no_rollback`
Expected: PASS（回滚重载 `fake://a`、Seek 12s、phase Failed、队列 current==0；首次失败无回滚）。

- [ ] **Step 7: 全量 + Commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

```bash
git add crates/hmp-daemon/src/engine.rs
git commit -m "feat(daemon): load commit order - media swap only after driver ACK; rollback reload on failure"
```

---

### Task 5: 显式 PlaybackSession——event id 精确闭合 + listened_ms 用旧曲位置

**Files:**
- Modify: `crates/hmp-storage/src/db.rs`（record_play_start 返回 id、record_play_end 按 event_id）
- Modify: `crates/hmp-daemon/src/engine.rs`（PlaybackSession、start/end/close_session、换曲调用点 4 处）
- Test: `crates/hmp-storage/src/db.rs` tests + `crates/hmp-daemon/src/engine.rs` tests

**Interfaces:**
- Consumes: Task 4 的 load_and_play（成功后调 start_session）。
- Produces:
  - `LibraryDb::record_play_start(&mut self, track_id: i64, started_at: i64) -> rusqlite::Result<i64>`（返回新事件 id）
  - `LibraryDb::record_play_end(&mut self, event_id: i64, e: &PlayEnd) -> rusqlite::Result<()>`（按 id 闭合，`WHERE id = ?1 AND ended_at IS NULL`；仍单事务含 play_count 累加）
  - `struct PlaybackSession { track_id: i64, event_id: i64 }`（engine.rs 私有）
  - `PlaybackEngine.session: Option<PlaybackSession>`（替代 `current_db_track: Option<i64>`）
  - `fn start_session(&mut self, track)` 每次播放动作新建会话（**删除同曲延续 return**）
  - `fn end_session(&mut self, reason)` / `fn close_session(&self, s: &PlaybackSession, reason: &'static str, listened_ms: i64)`
  - 换曲路径（play_source、navigate_next、navigate_prev、QueueRemove current 分支）：装载**前**捕获 `old_session = self.session.clone()` 与 `old_position = self.state_rx.borrow().position`；成功后 `close_session(&old, reason, old_position_ms)`（不再需要同曲 != 判断）

- [ ] **Step 1: storage 测试先行**

`crates/hmp-storage/src/db.rs` tests 模块追加（找现有 play_events 测试区，如 `recent_plays`/`play_count 累加` 测试附近，约 1479-1530 行）：

```rust
    #[test]
    fn play_start_returns_id_and_end_closes_by_id() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.upsert_track(&TrackRow {
            source: "local".into(),
            source_key: "local:/x.mp3".into(),
            title: "x".into(),
            album: None,
            artist: None,
            duration_ms: Some(60_000),
            cover_uri: None,
            qq_song_id: None,
        })
        .unwrap();
        let tid = 1;
        let id1 = db.record_play_start(tid, 1000).unwrap();
        let id2 = db.record_play_start(tid, 2000).unwrap();
        assert_ne!(id1, id2, "每次开始都返回独立事件 id");
        // 按 id1 精确闭合：只影响第一条。
        db.record_play_end(
            id1,
            &PlayEnd { track_id: tid, ended_at: 3000, listened_ms: 500, reason: "ended" },
        )
        .unwrap();
        let (open_count, closed_reason) = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM play_events WHERE ended_at IS NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            .pipe(|n| (n, ""));
        assert_eq!(open_count, 1, "只有第二条仍 open");
        // 重复闭合同 id：幂等（updated=0，play_count 不重复累加）。
        db.record_play_end(
            id1,
            &PlayEnd { track_id: tid, ended_at: 3000, listened_ms: 500, reason: "ended" },
        )
        .unwrap();
        let pc: i64 = db
            .conn
            .query_row("SELECT play_count FROM tracks WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pc, 1, "重复闭合不重复累加");
        let _ = closed_reason;
    }
```

注意：`TrackRow` 字段与现有构造一致（对照现有测试里的 `TrackRow { … }` 用法）；`db.conn` 若为私有字段，改用现有公开查询方法（如 `recent_plays`）或测试模块内直接访问（同文件测试可访问私有）。`.pipe` 不需要——直接两步写。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-storage play_start_returns_id_and_end_closes_by_id`
Expected: FAIL（编译失败：`record_play_start` 返回 `()`；或行为失败：按 track 猜闭合导致两条都闭合）。

- [ ] **Step 3: 改 storage 签名**

`crates/hmp-storage/src/db.rs`：

```rust
    /// 开启播放会话，返回事件 id（供结束按 id 精确闭合——同一曲目连续
    /// 播放产生独立会话，不再按 track_id 猜测）。
    pub fn record_play_start(&mut self, track_id: i64, started_at: i64) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO play_events (track_id, started_at) VALUES (?1, ?2)",
            params![track_id, started_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 结束播放会话：按事件 id 精确闭合（同曲重播各自独立闭合），
    /// 累加播放次数与最近播放时间（两 SQL 同事务：历史闭合失败则计数不更新）。
    pub fn record_play_end(&mut self, event_id: i64, e: &PlayEnd) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            let updated = self.conn.execute(
                r#"UPDATE play_events SET ended_at = ?2, listened_ms = ?3, end_reason = ?4
                   WHERE id = ?1 AND ended_at IS NULL"#,
                params![event_id, e.ended_at, e.listened_ms, e.reason],
            )?;
            if updated > 0 {
                self.conn.execute(
                    "UPDATE tracks SET play_count = play_count + 1, last_played_at = ?2 WHERE id = ?1",
                    params![e.track_id, e.ended_at],
                )?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(err) => {
                self.conn.execute_batch("ROLLBACK").ok();
                return Err(err);
            }
        }
        Ok(())
    }
```

适配现有测试中 `record_play_end(&end)` 的调用点（传入从 `record_play_start` 拿到的 id）；`record_play_start` 现有调用点忽略返回值处改为 `let _ = …` 或接收 id。

- [ ] **Step 4: engine 测试先行（session 语义）**

tests 模块追加（需要 library 注入——现有测试是否有注入先例：搜索 `start_with_library` 在 tests 的用法；若无，直接调用）：

```rust
    fn memory_library() -> Arc<Mutex<hmp_storage::LibraryDb>> {
        Arc::new(Mutex::new(hmp_storage::LibraryDb::open_in_memory().unwrap()))
    }

    #[tokio::test]
    async fn manual_change_closes_old_session_with_old_position() {
        // 手动换曲：旧会话闭合用换曲时刻的旧位置（而非新曲刚装载的 ~0）。
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![track_a(), track_b()]]);
        let lib = memory_library();
        let handle = PlaybackEngine::start_with_library(driver.clone(), resolver, || true, Some(lib.clone()));
        handle.cmd(Request::Play(PlayRequest::Track(track_a()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // 位置前进到 90s（模拟播放中）。
        driver.state_tx.send_modify(|s| s.position = std::time::Duration::from_secs(90));
        handle.cmd(Request::Play(PlayRequest::Track(track_b()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let mut lib = lib.lock().unwrap();
        // 直接查 play_events（同文件测试可访问私有 conn）。
        let mut stmt = lib.conn.prepare(
            "SELECT track_id, ended_at, listened_ms, end_reason FROM play_events ORDER BY id",
        ).unwrap();
        let evs: Vec<(i64, Option<i64>, i64, String)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(evs.len(), 2, "两条独立会话");
        assert_eq!(evs[0].0, evs[1].0, "同曲重播也各自独立（此处 a、b 不同曲，track 不同）");
        assert!(evs[0].1.is_some(), "旧会话已闭合");
        assert_eq!(evs[0].2, 90_000, "旧会话 listened_ms 用换曲时刻位置");
        assert_eq!(evs[0].3, "manual");
        assert!(evs[1].1.is_none(), "新会话保持 open");
    }

    #[tokio::test]
    async fn replay_same_track_creates_two_sessions() {
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![track_a()]]);
        let lib = memory_library();
        let handle = PlaybackEngine::start_with_library(driver.clone(), resolver, || true, Some(lib.clone()));
        handle.cmd(Request::Play(PlayRequest::Track(track_a()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        driver.state_tx.send_modify(|s| s.position = std::time::Duration::from_secs(30));
        handle.cmd(Request::Play(PlayRequest::Track(track_a()))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let mut lib = lib.lock().unwrap();
        let mut stmt = lib.conn.prepare(
            "SELECT ended_at, listened_ms, end_reason FROM play_events ORDER BY id",
        ).unwrap();
        let evs: Vec<(Option<i64>, i64, String)> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(evs.len(), 2, "同曲重播 = 两条独立会话（不再按 track 延续合并）");
        assert!(evs[0].0.is_some() && evs[0].1 == 30_000 && evs[0].2 == "manual");
        assert!(evs[1].0.is_none());
    }
```

若 `recent_plays` 的返回结构不可用或 conn 为私有但同模块可访问——tests 是 `mod tests` 内，`use super::*` 后私有字段可访问（现有测试直接用了 `db.conn`？查现有测试模式：1479 行附近有直接 SQL——沿用）。

- [ ] **Step 5: 跑 engine 测试确认失败**

Run: `cargo test -p hmp-daemon --lib manual_change_closes_old_session_with_old_position`
Expected: FAIL（现有实现按 track 猜闭合 + listened_ms 用新曲位置 → 断言 90_000 失败或闭合错误）。

- [ ] **Step 6: engine 实现 session**

`PlaybackEngine` 字段：`current_db_track: Option<i64>` → `session: Option<PlaybackSession>`；初始化 `session: None`。

```rust
/// 当前播放会话（媒体库写回锚点：event id 精确闭合）。
#[derive(Clone)]
struct PlaybackSession {
    track_id: i64,
    event_id: i64,
}
```

三个方法替换：

```rust
    /// 媒体库：upsert 曲目并开启播放会话（B4 会话粒度：INSERT play_events 返回 event id）。
    /// 每次播放动作独立会话（同曲重播也新建——listened_ms 各自记录）。
    fn start_session(&mut self, track: &hmp_core::Track) {
        let Some(library) = &self.library else {
            return;
        };
        let mut library = library.lock().unwrap();
        let row = track_row(track);
        match library.upsert_track(&row) {
            Ok(track_id) => match library.record_play_start(track_id, now_unix()) {
                Ok(event_id) => {
                    self.session = Some(PlaybackSession { track_id, event_id });
                }
                Err(e) => tracing::warn!(%e, "媒体库会话开启失败"),
            },
            Err(e) => tracing::warn!(%e, "媒体库 upsert 失败"),
        }
    }

    /// 媒体库：结束当前播放会话（按 event id 精确闭合 + 播放次数）。
    /// 收听时长 = 当前播放位置（位置无时长上限时原样记录）。
    fn end_session(&mut self, reason: &'static str) {
        if let Some(s) = self.session.take() {
            let listened_ms = self.state_rx.borrow().position.as_millis() as i64;
            self.close_session(&s, reason, listened_ms);
        }
    }

    /// 按事件 id 关闭会话（事务提交路径用：换曲前捕获的旧位置作 listened_ms）。
    fn close_session(&self, s: &PlaybackSession, reason: &'static str, listened_ms: i64) {
        let Some(library) = &self.library else {
            return;
        };
        let mut library = library.lock().unwrap();
        let end = hmp_storage::PlayEnd {
            track_id: s.track_id,
            ended_at: now_unix(),
            listened_ms,
            reason,
        };
        if let Err(e) = library.record_play_end(s.event_id, &end) {
            tracing::warn!(%e, "媒体库会话结束回写失败");
        }
    }
```

- [ ] **Step 7: 改 4 处换曲调用点**

模式统一（以 navigate_next 为例；play_source、navigate_prev、QueueRemove current 分支同构）：

```rust
    async fn navigate_next(&mut self) {
        // 先裁决再换会话（P1：队列无可跳目标时不得先关掉当前会话）。
        let saved = self.queue.save_state();
        let Some(id) = self.queue.skip_next() else {
            return;
        };
        // 装载前捕获旧会话与旧位置（listened_ms 用换曲时刻位置，非新曲 ~0）。
        let old_session = self.session.clone();
        let old_position = self.state_rx.borrow().position;
        if self.load_and_play(id).await.is_ok() {
            if let Some(old) = old_session {
                self.close_session(&old, "next", old_position.as_millis() as i64);
            }
        } else {
            self.queue.restore_state(saved);
            self.restore_phase_after_failure();
        }
    }
```

- play_source：`let old_db_track = self.current_db_track;` → `let old_session = self.session.clone(); let old_position = self.state_rx.borrow().position;`；成功后 `if let Some(old) = old_session { self.close_session(&old, "manual", old_position.as_millis() as i64); }`（删除 `if self.current_db_track != Some(old)` 同曲判断）。
- navigate_prev：同上，reason 用 `"previous"`。
- QueueRemove current 分支（178 行附近）：同上，reason `"manual"`；空队列分支 `end_session("manual")` 不变。

- [ ] **Step 8: 跑 engine 测试 + storage 测试**

Run: `cargo test -p hmp-daemon --lib manual_change_closes_old_session_with_old_position replay_same_track_creates_two_sessions && cargo test -p hmp-storage play_start_returns_id_and_end_closes_by_id`
Expected: PASS。

- [ ] **Step 9: 全量 + Commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿（注意 e2e/daemon_cli 若有历史断言需适配——`listened_ms` 语义变化只影响值不改变闭合行为）。

```bash
git add crates/hmp-storage/src/db.rs crates/hmp-daemon/src/engine.rs
git commit -m "feat(storage,daemon): explicit PlaybackSession - record_play_start returns event id, end by id, listened_ms from pre-switch position"
```

---

### Task 6: 全量验证 + e2e 冒烟

**Files:**
- Test: workspace 全量 + e2e + daemon_cli

- [ ] **Step 1: 全量验证**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

- [ ] **Step 2: e2e + CLI 冒烟**

Run: `cargo test -p hmp-daemon --test e2e && cargo test -p hmp-cli --test daemon_cli -- --ignored`
Expected: 全绿。

- [ ] **Step 3: 核对计划覆盖**

对照 Global Constraints 与审计诉求逐项核对：
- 500ms 窗口已删除（Task 3）；旧代 EOS/Error 按 gen 忽略；同代 EOS 短曲不丢。
- 装载 ACK 后才提交（Task 4）：active_media 延迟替换、last_load 回滚、超时失败路径。
- 历史 listened_ms 用换曲前位置（Task 5）；同曲重播独立会话；event id 精确闭合；close_stale_sessions 已在批 1 落地（无需重复）。

- [ ] **Step 4: 汇总报告**

报告：changed files、每任务验证结果、`cargo test --workspace` 总通过数、clippy/fmt 状态、未完成项。不要 git push（父会话统一推送）。

---

## Self-Review Notes（编写时已核对）

- **Spec coverage**：审计 P1 三项（listened_ms 记错 / 事务式换曲不覆盖装载 / 无 generation）+ 播放历史同曲合并、双 SQL 事务（已批 1 完成，Task 5 强化为按 id）全部有对应任务。
- **类型一致性**：`gen: u64` 贯穿 PlaybackState/LoadRequest/PlayerEvent/engine.current_gen/AppliedLoad.gen；`record_play_start -> Result<i64>`、`record_play_end(event_id, &PlayEnd)`、`close_session(&PlaybackSession, reason, listened_ms)` 签名在 Task 1/5 定义处与使用处一致；`start_with_options` 在 Task 4 定义、Task 4 测试使用。
- **占位符**：所有代码块为实际可编译内容；测试 helper（`track_a()` 等）注明"沿用 tests 模块已有 helper"，因现有测试已有同名构造（`advance_on_eos` 等测试使用），实现者需确认现有名称。
