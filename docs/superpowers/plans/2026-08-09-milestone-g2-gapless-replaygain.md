# Player Experience G2 Implementation Plan（里程碑 G，第 2 批）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 里程碑 G 第 2 批：gapless/preload（下一首预解析）+ ReplayGain（本地曲目音量补偿）。

**决策定案：**
1. **preload 指纹失效**：预解析结果缓存槽 `PreloadSlot { key: (queue_revision, current_gen), id, res }`。消费时 `slot.key == (queue.revision(), current_gen) && slot.id == id` 才命中；队列任何结构变更（revision+1）或换代（gen+1）天然失配，**无需显式清缓存**。写入时防乱序覆盖：仅当 `slot 为空 或 slot.key <= 我的 key`（revision 单调、gen 单调，字典序比较）才写入——旧任务不得覆盖新任务结果。
2. **ResolvedTrack 直接 move 消费**（不用 Arc）：slot 存 owned `ResolvedTrack`，命中后 `take()` 移出（media guard 随之移交，无克隆问题）。
3. **预解析失败静默**：只 `tracing::debug`，不发布 last_error、不影响播放；装载时缓存未命中就走正常 resolve_track 路径（错误语义不变）。
4. **预解析触发点**：仅 `load_and_play` 成功路径末尾（phase=Playing、start_session 后）。队列空/无下一首（peek_next None）不触发。
5. **ReplayGain 应用**：引擎持 `user_volume`（用户音量，初始 = 恢复会话音量或 1.0）与 `rg_factor`（当前曲增益因子）。装载成功后 `driver.set_volume(user_volume * rg_factor)`；`SetVolume(v)` 拦截：`user_volume = v; driver.set_volume(v * rg_factor)`（用户调音量不丢补偿，换曲自动切换增益）。
6. **RG 因子**：`factor = 10^(db/20)`，clamp 到 [0.25, 4.0]（±12dB，防异常标签）。配置开关 `[audio] replaygain`（bool，serde default true）；装载时读 `Config::load()`（与音质策略每次 resolve 读配置一致，player.rs:295）。
7. **范围**：仅本地曲目有 RG（QQ 无标签源 → None → factor 1.0）。不做命令行设置命令（config.toml 人工编辑）。

**Architecture:**
- `hmp-core/src/queue.rs`：`QueueCore::peek_next(&self) -> Option<TrackId>`（只读；与 `advance_on_eos` 返回一致但不移动 cursor、不递增 revision）。
- `hmp-daemon/src/engine.rs`：`PreloadSlot` + `preload_slot: Arc<tokio::sync::Mutex<Option<PreloadSlot>>>` + `schedule_preload()`（tokio::spawn 后台 resolve_track）+ `load_and_play` 缓存消费；`user_volume`/`rg_factor` 字段 + `apply_gain`；`handle_player_command` 拦截 SetVolume。
- `hmp-storage/src/local.rs`：`LocalMeta.replaygain_track_db: Option<f64>` + `parse_rg_db`（lofty `ItemKey::ReplayGainTrackGain` 文本如 "-6.50 dB" → f64）。
- `hmp-storage/src/config.rs`：`AudioPref.replaygain: bool`（default true）。
- `hmp-daemon/src/player.rs`：`ResolvedTrack.replaygain_db: Option<f64>`（QQ 构造点补 None；本地 resolve_local 填 meta 值）。

**Tech Stack:** Rust workspace（hmp-core / hmp-storage / hmp-daemon）；tokio spawn。

## Global Constraints

- 保持 `cargo test --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check`、e2e、daemon_cli（--ignored）全绿。
- preload 不得改变任何公开行为语义：装载失败/回滚/会话/seq 逻辑与现有完全一致，仅跳过网络往返。
- `peek_next` 必须与 `advance_on_eos` 返回值一致（每个分支对应验证）。
- RG 增益在 `SetVolume` 后仍叠加（用户音量与补偿分离）；`rg_factor` 只在装载成功路径更新。
- 每个 Task 独立 commit。

---

### Task 1: 队列 peek_next + 引擎 preload 预解析（TDD）

**Files:**
- Modify: `crates/hmp-core/src/queue.rs`（`peek_next`）
- Modify: `crates/hmp-daemon/src/engine.rs`（`PreloadSlot`/`schedule_preload`/`load_and_play` 消费；FakeResolver 扩展）
- Test: `crates/hmp-core/src/queue.rs` tests + `crates/hmp-daemon/src/engine.rs` tests

**Interfaces:**
- Consumes: `QueueCore::revision()`（已有）。
- Produces:
  - `pub fn peek_next(&self) -> Option<TrackId>`：与 `advance_on_eos` 返回一致（空→None；`LoopMode::Track` 且 has_current→当前曲；`!has_current`→`order[0]` 首曲；`cursor+1 >= len` 时 None 模式→None、List/Track 回绕→`order[0]`；否则 `order[cursor+1]`）。**不移动 cursor、不递增 revision**。
  - `struct PreloadSlot { key: (u64, u64), id: TrackId, res: ResolvedTrack }`（engine 私有）。
  - `PlaybackEngine` 字段 `preload_slot: Arc<tokio::sync::Mutex<Option<PreloadSlot>>>`。
  - `fn schedule_preload(&self)`：peek_next → None 则 return；key=(queue.revision(), current_gen)；`tokio::spawn` 闭包：resolve_track(next) → Ok → 写槽（乱序保护：`slot 为空 || slot.key <= key` 才写，见决策 1）；Err → `tracing::debug!`。
  - `load_and_play` 开头：缓存命中检查（决策 1 条件）→ `slot.take()` 移出 ResolvedTrack → 走与现有 `Ok(res)` 相同的装载路径（提取 `let res = cached.or_else(|| resolve_track 的结果)` 结构；现有 `Ok(res)` 分支体不变）；未命中 → 原路径。
  - `load_and_play` 成功路径末尾（start_session 后、publish 前）：`self.schedule_preload();`。

- [ ] **Step 1: 写队列测试（先行）**

`crates/hmp-core/src/queue.rs` tests（现有 advance/prev 测试附近）：

```rust
    /// G2：peek_next 与 advance_on_eos 返回一致，且不移动 cursor / 不递增 revision。
    #[test]
    fn peek_next_matches_advance_without_mutation() {
        let mut q = QueueCore::new();
        q.replace(vec![t("a"), t("b"), t("c")], 0);
        assert_eq!(q.peek_next(), Some(t("b")));
        assert_eq!(q.peek_next(), Some(t("b"))); // 幂等
        assert_eq!(q.revision(), 0, "peek 不得变更结构");
        assert_eq!(q.advance_on_eos(), Some(t("b")));
        assert_eq!(q.peek_next(), Some(t("c")));

        // 末尾 + None 模式：advance 返回 None，peek 亦 None。
        q.advance_on_eos();
        assert_eq!(q.peek_next(), None);
        assert_eq!(q.advance_on_eos(), None);

        // List 回绕：peek 也回绕到首曲。
        q.set_loop_mode(LoopMode::List);
        assert_eq!(q.peek_next(), Some(t("a")));
        assert_eq!(q.advance_on_eos(), Some(t("a")));

        // Repeat One：peek 当前曲（重播）。
        q.set_loop_mode(LoopMode::Track);
        assert_eq!(q.peek_next(), Some(t("a")));

        // 无当前曲：peek 首曲。
        let mut q2 = QueueCore::new();
        q2.replace(vec![t("x"), t("y")], 0);
        q2.clear();
        q2.replace(vec![t("x"), t("y")], 0);
        // clear 后 has_current=false：重建队列后 peek 应为首曲。
        assert_eq!(q2.peek_next(), Some(t("x")));

        // 空队列。
        let q3 = QueueCore::new();
        assert_eq!(q3.peek_next(), None);
    }
```

（`t()` helper 与 `set_loop_mode`/`replace` 签名以现有测试为准——先读 queue.rs 测试段的实际写法，必要时适配。`clear` 后 replace 的 has_current 语义以 `skip_next` 的 `!has_current` 分支行为为准，测试与实现同步核对。）

- [ ] **Step 2: 实现 peek_next + 队列测试通过**

Run: `cargo test -p hmp-core peek_next && cargo test -p hmp-core --lib`
Expected: 全绿。

- [ ] **Step 3: 写引擎测试（先行）**

`crates/hmp-daemon/src/engine.rs` tests。**先读现有 FakeResolver 实现**（约 1080 行 `/// 固定返回曲目列表的解析器（不触网）`）与 `wait_idle` helper，按其模式扩展：

```rust
    /// 里程碑 G2：装载后后台预解析下一首；Next 时直接消费缓存（不二次 resolve）。
    #[tokio::test]
    async fn preloads_next_track_and_consumes_cache() {
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
        let handle = PlaybackEngine::start(driver.clone(), Arc::new(resolver.clone()), Arc::new(|| true));
        handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("a")))).await.unwrap();
        wait_idle().await;
        // 等后台预解析完成。
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(resolver.resolve_calls(), 2, "播放 a 后应预解析 b");
        handle.cmd(Request::Command(PlayerCommand::Next)).await.unwrap();
        wait_idle().await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(resolver.resolve_calls(), 2, "Next 到 b 应消费预解析缓存，不再次 resolve");
        assert_eq!(handle.state_rx.borrow().playback.current.as_ref().map(|t| t.id.as_ref()), Some("b"));
    }

    /// 队列变更（Play 新源）后缓存失效：Next 走正常 resolve。
    #[tokio::test]
    async fn preload_cache_invalidated_by_queue_change() {
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![
            vec![TrackId::new("a"), TrackId::new("b")],
            vec![TrackId::new("c"), TrackId::new("d")],
        ]);
        let handle = PlaybackEngine::start(driver.clone(), Arc::new(resolver.clone()), Arc::new(|| true));
        handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("a")))).await.unwrap();
        wait_idle().await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(resolver.resolve_calls(), 2); // a + 预解析 b
        // 队列整体替换 → revision 变更。
        handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("c")))).await.unwrap();
        wait_idle().await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        // c 装载 + 预解析 d。
        assert_eq!(resolver.resolve_calls(), 4);
        handle.cmd(Request::Command(PlayerCommand::Next)).await.unwrap();
        wait_idle().await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(resolver.resolve_calls(), 4, "d 已预解析，Next 不新增 resolve");
    }

    /// 预解析失败静默：播放不受影响，Next 时走正常路径（失败语义不变）。
    #[tokio::test]
    async fn preload_failure_is_silent_and_falls_back() {
        // FakeResolver 扩展：指定某 id resolve_track 返回 Err（见实现步骤），
        // 其余正常。b 解析失败 → 播放 a 正常（无 last_error）；
        // Next → load_and_play(b) 走 resolve（失败 → Failed 阶段 + last_error）。
    }
```

（FakeResolver 需要：`#[derive(Clone)]` + 共享调用计数 `Arc<Mutex<usize>>` + `resolve_calls()` 访问器 + 可配置失败 id。以现有实现为准最小扩展——若现有 FakeResolver 已 Clone/计数则直接复用。）

- [ ] **Step 4: 跑引擎测试确认失败**

Run: `cargo test -p hmp-daemon preloads_next_track_and_consumes_cache`
Expected: FAIL（无预解析：resolve_calls 停在 1）。

- [ ] **Step 5: 实现引擎 preload**

1. `PreloadSlot` 结构 + `PlaybackEngine.preload_slot` 字段（`Arc<tokio::sync::Mutex<Option<PreloadSlot>>>`；start 时初始化）。
2. `schedule_preload(&self)`（决策 1/3/4）。
3. `load_and_play` 开头缓存消费（决策 2：`take()` 移出）；`Ok(res)` 分支体不动。
4. `load_and_play` 成功路径末尾 `self.schedule_preload();`。

- [ ] **Step 6: 跑测试确认通过 + 全量**

Run: `cargo test -p hmp-daemon preload_ && cargo test -p hmp-daemon --lib && cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

- [ ] **Step 7: Commit**

```bash
git add crates/hmp-core/src/queue.rs crates/hmp-daemon/src/engine.rs
git commit -m "feat(core,daemon): preload next track - peek_next + background resolve cache with revision/gen fingerprint"
```

---

### Task 2: ReplayGain 本地曲目音量补偿（TDD）

**Files:**
- Modify: `crates/hmp-storage/src/local.rs`（`parse_rg_db` + `LocalMeta.replaygain_track_db`）
- Modify: `crates/hmp-storage/src/config.rs`（`AudioPref.replaygain: bool` default true）
- Modify: `crates/hmp-daemon/src/player.rs`（`ResolvedTrack.replaygain_db`；QQ resolve 构造点补 None；`local.rs` 的 `resolve_local` 填值——注意 local.rs 在 hmp-daemon，ResolvedTrack 也在 hmp-daemon）
- Modify: `crates/hmp-daemon/src/engine.rs`（`user_volume`/`rg_factor`/`apply_gain`/SetVolume 拦截；FakeResolver 扩展）
- Test: `crates/hmp-storage/src/local.rs` tests + `crates/hmp-daemon/src/engine.rs` tests

**Interfaces:**
- Consumes: `ItemKey::ReplayGainTrackGain`（lofty 0.21，字符串形式 `-6.50 dB`）。
- Produces:
  - `pub fn parse_rg_db(s: &str) -> Option<f64>`：解析 `-6.50 dB` / `+3.0 dB` / `0 dB` / `12.34 dB`（无符号=正）；失败/乱串 → None。实现：去空白、去尾部 "dB"（大小写不敏感）、trim、`parse::<f64>()` 兜底。
  - `LocalMeta.replaygain_track_db: Option<f64>`（read_meta 里 `tag.and_then(|t| t.get_string(&ItemKey::ReplayGainTrackGain)).and_then(parse_rg_db)`）。
  - `ResolvedTrack.replaygain_db: Option<f64>`；构造点：QQ `resolve_track_impl` 补 `replaygain_db: None`；local `resolve_local` 填 `meta.as_ref().and_then(|m| m.replaygain_track_db)`。
  - `AudioPref.replaygain: bool`（`#[serde(default = "default_true")]`；`default_true` 已有——quality.rs 里是 hmp-cli 的；config.rs 里 `fn default_true()` 已存在 ✓ 检查后用同一个）。
  - engine：`user_volume: f64`（初始：恢复路径 restored_volume 同步；否则 1.0）、`rg_factor: f64`（1.0）、`fn apply_gain(&mut self, res: &ResolvedTrack)`（决策 5/6：读 `Config::load().audio.replaygain`；false → factor 1.0；true → `res.replaygain_db.map(...)` clamp；`driver.set_volume(user_volume * rg_factor)`）；`load_and_play` 成功路径 `self.apply_gain(&res)`（schedule_preload 前）；`handle_player_command` 加分支 `PlayerCommand::SetVolume(v) => { self.user_volume = v; self.driver.command(PlayerCommand::SetVolume(v * self.rg_factor)); }`（**替换** default 直通）。

- [ ] **Step 1: 写 parse_rg_db 测试（先行）**

`crates/hmp-storage/src/local.rs` tests：

```rust
    #[test]
    fn parses_replaygain_db() {
        assert_eq!(parse_rg_db("-6.50 dB"), Some(-6.5));
        assert_eq!(parse_rg_db("+3.0 dB"), Some(3.0));
        assert_eq!(parse_rg_db("0 dB"), Some(0.0));
        assert_eq!(parse_rg_db("12.34dB"), Some(12.34));
        assert_eq!(parse_rg_db("-23.83 db"), Some(-23.83));
        assert_eq!(parse_rg_db(""), None);
        assert_eq!(parse_rg_db("abc"), None);
        assert_eq!(parse_rg_db("NaN dB"), None);
    }
```

- [ ] **Step 2: 实现 + 通过**

Run: `cargo test -p hmp-storage parses_replaygain_db`
Expected: 全绿。

- [ ] **Step 3: 写引擎测试（先行）**

`crates/hmp-daemon/src/engine.rs` tests（FakeResolver 扩展：按 id 返回带 `replaygain_db` 的 ResolvedTrack）：

```rust
    /// 里程碑 G2：RG 增益在装载时叠加到用户音量。
    #[tokio::test]
    async fn replaygain_applied_on_load() {
        // FakeResolver：a 带 replaygain_db=Some(6.0)（factor ≈ 1.995），b 无 RG。
        // Play a → state.volume ≈ 1.995（FakeDriver.set_volume 反映到 state）。
        // SetVolume(0.5) → state.volume ≈ 0.5 * 1.995。
        // Next → b（factor 1.0）→ state.volume == 0.5。
    }

    /// clamp：异常标签（+30dB → factor 封顶 4.0）。
    #[tokio::test]
    async fn replaygain_clamps_extreme_values() { … }

    /// 配置 [audio] replaygain=false 时 factor 恒 1.0（XDG_CONFIG_HOME 隔离）。
    #[tokio::test]
    async fn replaygain_disabled_by_config() { … }
```

（引擎测试读 Config 会碰真实 XDG_CONFIG_HOME——**必须隔离**：仿 quality.rs 的 isolated 模式（临时 XDG_CONFIG_HOME + 写 config.toml）；engine.rs 测试是否已有 env 隔离基建？若无，测试内 `std::env::set_var("XDG_CONFIG_HOME", …)` + 串行锁，或把配置读取注入（`PlaybackEngine::start` 已有 `Arc<dyn Fn() -> bool>` 网络守卫——**更干净：加一个注入点太改动，用 env 隔离即可**，注意与并行测试的互斥——engine 测试默认并行，env 隔离需 TEST_ENV_LOCK 串行化该测试。）

- [ ] **Step 4: 跑引擎测试确认失败**

Run: `cargo test -p hmp-daemon replaygain_applied_on_load`
Expected: FAIL。

- [ ] **Step 5: 实现**

1. hmp-daemon/player.rs：`ResolvedTrack.replaygain_db` 字段 + 所有构造点（grep `ResolvedTrack {` 全量补齐：QQ resolve_track_impl、local resolve_local、FakeResolver/其他测试构造）。
2. hmp-daemon/local.rs `resolve_local`：填 `replaygain_db: meta.as_ref().and_then(|m| m.replaygain_track_db)`。
3. engine：`user_volume`/`rg_factor` 字段 + 恢复路径同步 + `apply_gain` + SetVolume 分支 + load_and_play 成功路径调用。
4. hmp-storage/config.rs：`AudioPref.replaygain`。

- [ ] **Step 6: 跑测试确认通过 + 全量**

Run: `cargo test -p hmp-storage && cargo test -p hmp-daemon && cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e`
Expected: 全绿。

- [ ] **Step 7: Commit**

```bash
git add crates/hmp-storage/src/local.rs crates/hmp-storage/src/config.rs crates/hmp-daemon/src/player.rs crates/hmp-daemon/src/local.rs crates/hmp-daemon/src/engine.rs
git commit -m "feat(storage,daemon): replaygain for local tracks - parse RG tag, composite user volume x track gain"
```

---

### Task 3: 收尾验证

- [ ] **Step 1: 全量验证**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e && cargo test -p hmp-cli --test daemon_cli -- --ignored`
Expected: 全绿。

- [ ] **Step 2: 核对覆盖**

G2 验收：preload（peek_next 与 advance 一致；装载后后台预解析；Next/EOS 消费缓存零额外 resolve；队列变更/换代失效；预解析失败静默回退；Repeat One 预解析当前曲）；ReplayGain（parse_rg_db；LocalMeta/ResolvedTrack 字段；装载应用 user_volume × factor；SetVolume 叠加；clamp ±12dB；配置开关；QQ 曲目 None→1.0）。未做（超范围，记录）：MPRIS/CLI 的 RG 展示、album gain、峰值防削波。

- [ ] **Step 3: 报告**

报告：changed files、每任务验证（测试名+结果）、`cargo test --workspace` 总通过数、clippy/fmt 状态、与计划的偏差及原因、未完成项。不要 git push（父会话统一推送并更新 roadmap）。
