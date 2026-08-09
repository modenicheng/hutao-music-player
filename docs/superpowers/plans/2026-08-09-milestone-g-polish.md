# Player Experience Polish Plan（里程碑 G 打磨批）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 里程碑 G 后的 3 项打磨：Repeat One 跳过冗余预解析、CLI status 展示 RG 增益、`hmp serve --sink` 命令行覆盖。

**决策定案：**
1. **Repeat One 跳过预解析**：`schedule_preload` 开头加 `loop_mode()==Track` 守卫直接 return（Track 模式 EOS 重播当前曲，预解析当前曲会重复 resolve + 创建重复解密代理；手动 Next 不受 Repeat One 影响但用户主动操作可接受正常 resolve 延迟）。**不改 `peek_next`**（其与 advance_on_eos 的一致性是被 review 验证的性质，G2 打磨不动）。
2. **RG 展示**：`DaemonState` 加 `replaygain_db: Option<f64>`（当前曲 RG 标签 dB；无/QQ → None）；engine 持 `current_rg_db`（apply_gain 时记录）；CLI `hmp status` 在音质行下加"增益: +6.0 dB / （无）"。**不做 MPRIS 展示**（MPRIS 无 RG 标准字段，避免非标扩展，记录理由）。
3. **`serve --sink`**：CLI `Serve { background, sink: Option<String> }`；`run_foreground(sink)`/`run_background(sink)`；`spawn_detached` args 构造（`["serve", "--sink", name]`）；`merge_audio_sink` 注入优先逻辑已就位（G1）——DaemonConfig.audio_sink 直通即可。后台 detached 传参经命令行，无环境变量传递问题。

**Architecture:**
- `crates/hmp-core/src/ipc.rs`：`DaemonState.replaygain_db: Option<f64>`（serde 自动；`#[serde(default)]` 不需要——daemon/cli 同 crate 版本）。
- `crates/hmp-daemon/src/engine.rs`：`current_rg_db: Option<f64>` 字段（apply_gain 记录）；publish 填 DaemonState；schedule_preload Track 守卫。
- `crates/hmp-cli/src/commands.rs`：format_status 增益行；测试构造点补字段。
- `crates/hmp-cli/src/main.rs` + `crates/hmp-daemon/src/serve.rs`：--sink 参数链。

**Tech Stack:** Rust workspace（hmp-core / hmp-daemon / hmp-cli）；clap。

## Global Constraints

- 保持 `cargo test --workspace`、clippy `-D warnings`、fmt、e2e、daemon_cli（--ignored）全绿。
- `peek_next`/advance_on_eos **不改**。
- `DaemonState` 所有构造点（engine.rs:476、ipc.rs:450、commands.rs:406/495、e2e.rs:452）补字段——grep `DaemonState {` 全量。
- RG 展示值 = 标签原始 dB（未 clamp 值；engine 已存的是 clamp 前的 replaygain_db）。
- 每个 Task 独立 commit。

---

### Task 1: Repeat One 跳过冗余预解析（TDD）

**Files:**
- Modify: `crates/hmp-daemon/src/engine.rs`（schedule_preload 守卫）
- Test: engine.rs tests

- [ ] **Step 1: 写测试（先行）**

engine.rs tests（现有 preload 测试附近）：

```rust
    /// 打磨：Repeat One（Track 模式）EOS 重播当前曲——预解析当前曲是冗余
    /// 的（重复 resolve + 重复解密代理），应跳过。
    #[tokio::test]
    async fn preload_skipped_in_repeat_one() {
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a"), TrackId::new("b")]]);
        let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
        handle.cmd(Request::Command(PlayerCommand::SetLoopMode(LoopMode::Track))).await.unwrap();
        handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("a")))).await.unwrap();
        wait_idle().await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        // 装载 a（1 次 resolve）；Track 模式不预解析（保持 1）。
        assert_eq!(resolver.resolve_calls(), 1, "Repeat One 不应预解析当前曲");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-daemon preload_skipped_in_repeat_one`
Expected: FAIL（当前 resolve_calls==2）。

- [ ] **Step 3: 实现**

`schedule_preload` 开头（`loaded == current()` 守卫之后或之前均可，语义等价）：

```rust
        // 打磨：Repeat One（Track）EOS 重播当前曲——预解析当前曲冗余
        //（重复 resolve + 重复解密代理），跳过；手动 Next 不受影响
        //（用户主动操作走正常 resolve 延迟可接受）。
        if self.queue.loop_mode() == hmp_core::LoopMode::Track {
            return;
        }
```

- [ ] **Step 4: 通过 + 全量**

Run: `cargo test -p hmp-daemon preload_ && cargo test -p hmp-daemon --lib && cargo test --workspace && clippy && fmt`
Expected: 全绿。

- [ ] **Step 5: Commit**

```bash
git commit -m "perf(daemon): skip preload in Repeat One - replaying current track needs no background resolve"
```

---

### Task 2: CLI status 展示 RG 增益（TDD）

**Files:**
- Modify: `crates/hmp-core/src/ipc.rs`（DaemonState.replaygain_db）
- Modify: `crates/hmp-daemon/src/engine.rs`（current_rg_db + publish）
- Modify: `crates/hmp-cli/src/commands.rs`（format_status + 测试构造点）
- Modify: `crates/hmp-daemon/tests/e2e.rs`（构造点，如需要）
- Test: engine.rs + commands.rs tests

- [ ] **Step 1: 写测试（先行）**

engine.rs（replaygain 测试附近）：

```rust
    /// 打磨：DaemonState 携带当前曲 RG 增益（CLI status 展示用）。
    #[tokio::test]
    async fn state_exposes_replaygain_db() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let (driver, _sr, _er) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("a")]]);
        resolver.replaygain.lock().unwrap().push((TrackId::new("a"), -6.5));
        let (handle, _st) = start_engine(driver.clone(), resolver.clone()).await;
        handle.cmd(Request::Play(PlayRequest::Track(TrackId::new("a")))).await.unwrap();
        wait_idle().await;
        let st = handle.state_rx.borrow().clone();
        assert_eq!(st.replaygain_db, Some(-6.5));
        // 无 RG 曲目 → None。
        handle.cmd(Request::Command(PlayerCommand::Next)).await.unwrap(); // b 无 RG
        wait_idle().await;
        assert_eq!(handle.state_rx.borrow().replaygain_db, None);
    }
```

（注意：`state_rx` 是 EngineHandle 的——engine publish 的 DaemonState watch。检查现有测试读 `handle.state_rx.borrow().playback...`——`replaygain_db` 加在 DaemonState 顶层。）

commands.rs tests（现有 `format_status` 测试附近）：

```rust
    #[test]
    fn status_shows_replaygain() {
        let st = sample_state(); // 现有测试构造 helper（补 replaygain_db 字段）
        let out = format_status(&st);
        assert!(out.contains("增益"));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Expected: FAIL（字段不存在 → 编译失败）。

- [ ] **Step 3: 实现**

1. ipc.rs：`DaemonState` 加 `pub replaygain_db: Option<f64>`（注释：当前曲 ReplayGain 标签 dB；无 → None）。
2. engine.rs：字段 `current_rg_db: Option<f64>`（init None）；`apply_gain` 里 `self.current_rg_db = replaygain_db;`（**注意**：apply_gain 现在接收 `Option<f64>` 参数——直接记录原值，不记录 clamp 后 factor）；publish 的 DaemonState 构造加 `replaygain_db: self.current_rg_db`。
3. commands.rs `format_status`：音质行后加：

```rust
    match st.replaygain_db {
        Some(db) => s.push_str(&format!("增益: {:+.1} dB\n", db)),
        None => s.push_str("增益: （无）\n"),
    }
```

4. 所有 `DaemonState {` 构造点补 `replaygain_db`（ipc.rs:450 测试、commands.rs:406/495 测试、engine.rs:476、e2e.rs:452——按上下文填 None 或测试值）。

- [ ] **Step 4: 通过 + 全量**

Run: `cargo test --workspace && clippy && fmt && e2e`
Expected: 全绿。

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(core,daemon,cli): expose current ReplayGain in status (DaemonState.replaygain_db + hmp status)"
```

---

### Task 3: `hmp serve --sink` 命令行覆盖（TDD）

**Files:**
- Modify: `crates/hmp-cli/src/main.rs`（Serve 参数 + 传参）
- Modify: `crates/hmp-daemon/src/serve.rs`（run_foreground/run_background 签名 + spawn_detached args）
- Modify: `crates/hmp-cli/tests/daemon_cli.rs`（真机集成测试，可选）
- Test: serve.rs 单测 + daemon_cli 集成

- [ ] **Step 1: 写测试（先行）**

serve.rs 单测（现有 merge_audio_sink 测试附近）：

```rust
    /// 打磨：--sink 命令行参数 → run_foreground 注入优先于 config.toml。
    #[test]
    fn serve_sink_injection_priority() {
        // merge_audio_sink(Some("pulsesink"), Some("fakesink".into())) → Some("pulsesink")
        // run_inner 的合并逻辑 G1 已验证；这里验证 run_foreground(sink) 参数
        // 能到达 DaemonConfig（通过注入优先组合断言即可，见现有测试）。
    }
```

daemon_cli.rs（真机项，`#[ignore]`，仿现有 library_playlist 测试）：

```rust
    /// 打磨：`hmp serve --background --sink fakesink` 应正常启动（fakesink 是
    /// 有效 GStreamer sink；真实音频环境无默认音频输出也能跑）。
    #[test]
    #[ignore = "需要真实 GStreamer 环境（真机验收项）"]
    fn serve_with_explicit_sink_starts() { … 与 library_playlist 同构，但 serve 参数带 --sink fakesink；
        启动成功 + quit 正常。 }
```

（若 daemon_cli 结构不便新增，serve.rs 单测 + clap 解析测试即可；真机项可选。）

- [ ] **Step 2: 跑测试确认失败**

Expected: FAIL（--sink 参数不存在 → clap 报错）。

- [ ] **Step 3: 实现**

1. main.rs：

```rust
    Serve {
        /// 后台模式（脱离终端）。
        #[arg(long)]
        background: bool,
        /// 音频输出 sink（GStreamer 元素名；覆盖 config.toml [audio] sink）。
        #[arg(long)]
        sink: Option<String>,
    },
```

```rust
        Command::Serve { background, sink } => {
            if background {
                hmp_daemon::serve::run_background(sink.as_deref()).await
            } else {
                hmp_daemon::serve::run_foreground(sink.as_deref()).await
            }
        }
```

2. serve.rs：

```rust
pub async fn run_foreground(sink: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    run_inner(DaemonConfig {
        audio_sink: sink.map(|s| s.to_string()),
    })
    .await
}

pub async fn run_background(sink: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = vec!["serve"];
    if let Some(s) = sink {
        args.push("--sink");
        args.push(s);
    }
    spawn_detached(&args)?;
    Ok(())
}
```

（注意：`spawn_detached(&args)`——args 是 Vec<&str>，`&args` 可转 &[&str] ✓；检查其他 run_background 调用点——CLI auto-spawn 也用 run_background()？grep 调用点全量更新。）

- [ ] **Step 4: 通过 + 全量**

Run: `cargo test --workspace && clippy && fmt && e2e && cargo test -p hmp-cli --test daemon_cli -- --ignored`
Expected: 全绿（真机项如失败且非本改动引入则记录）。

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(cli,daemon): hmp serve --sink overrides config.toml audio sink (CLI injection priority)"
```

---

### Task 4: 收尾验证

- [ ] **Step 1: 全量验证**

Run: `cargo test --workspace && clippy && fmt && e2e && daemon_cli --ignored`
Expected: 全绿。

- [ ] **Step 2: 核对覆盖**

打磨项：Repeat One 跳过预解析 ✓；CLI status 增益行（含"（无）"）✓；serve --sink 注入优先 ✓。不做：MPRIS RG 展示（无标准字段）、CLI 设置 RG 命令（config.toml 人工编辑）。

- [ ] **Step 3: 报告**

报告：changed files、每任务验证、总通过数、clippy/fmt 状态、偏差及原因、未完成项。不要 git push（父会话统一推送并更新 roadmap）。
