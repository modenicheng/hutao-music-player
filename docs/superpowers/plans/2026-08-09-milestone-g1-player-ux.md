# Player Experience G1 Implementation Plan（里程碑 G，第 1 批）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 里程碑 G（审计第 6 步收尾）第 1 批：Previous 曲首语义（position > 3s → 回曲首不换曲）+ 输出设备选择（config.toml `[audio] sink` → daemon 启动透传 GstDriver）。**G2（下轮）**：gapless/preload + ReplayGain。

**决策定案：**
1. Previous 阈值 3s（与主流播放器一致；`position > 3s` 时 `Seek(0)` 不换曲、不闭合会话）。
2. 输出设备：`Config` 加 `[audio] sink: Option<String>`（serde default None）；daemon 启动读配置传 `DaemonConfig.audio_sink`（与音质策略同模式——resolver 层读 `Config::load()`，见 player.rs:295）。**不加 CLI 设置命令**（config.toml 人工编辑；与 quality 的命令式管理不同步做，避免范围膨胀）。
3. 非法 sink 名：`GstDriver::new` 已返回 Err（元素创建失败）→ daemon 启动失败并报清晰错误（现状行为，不改）。

**Architecture:** engine `navigate_prev` 开头加曲首判断（driver `Seek(0)`；无队列/会话变更）；`hmp-storage::config::Config` 加 `AudioPref { sink: Option<String> }` 字段（`[audio]` toml 段）；`serve.rs` 读 Config 传给 `DaemonConfig`。

**Tech Stack:** Rust workspace（hmp-storage / hmp-daemon / hmp-player-gst）；tokio。

## Global Constraints

- 保持 `cargo test --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 全绿。
- `navigate_prev` 曲首路径**不**闭合会话、不推进 seq、不换曲（行为与"Seek 到 0"一致）。
- `Config` 向后兼容：无 `[audio]` 段的旧 config.toml → sink None（serde default）；`Config` 的 `PartialEq` derive 保留。
- 每个 Task 独立 commit（`feat(engine,…)` 前缀 + 中文要点）。

---

### Task 1: Previous 曲首语义（TDD）

**Files:**
- Modify: `crates/hmp-daemon/src/engine.rs`（`navigate_prev`）
- Test: `crates/hmp-daemon/src/engine.rs` tests

**Interfaces:**
- Produces（Task 2 无关，独立）:
  - `navigate_prev` 行为：position > 3s → `driver.command(Seek(ZERO))` 直接返回（不换曲、不闭合会话、不推进 seq）；否则现有逻辑。

- [ ] **Step 1: 写测试（先行）**

`crates/hmp-daemon/src/engine.rs` tests（现有 navigate 测试附近；用 FakeDriver 的 `commands` 记录与 `state_tx.send_modify` 推进位置——参考 1922 行测试的模式）：

```rust
    /// 里程碑 G：Previous 曲首语义——position > 3s 只回曲首，不换曲。
    #[tokio::test]
    async fn previous_restarts_track_when_past_three_seconds() {
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![
            vec![TrackId::new("a")],
            vec![TrackId::new("b")],
        ]);
        let handle = PlaybackEngine::start(driver.clone(), Arc::new(resolver), Arc::new(|| true));
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        // 位置推进到 >3s。
        driver
            .state_tx
            .send_modify(|s| s.position = std::time::Duration::from_secs(12));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let loads_before = driver.loads.lock().unwrap().len();
        handle.cmd(Request::Command(PlayerCommand::Previous)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // 不换曲：无新 load。
        assert_eq!(driver.loads.lock().unwrap().len(), loads_before, ">3s 时 Previous 不应换曲");
        // 收到 Seek(0)。
        let seeks: Vec<PlayerCommand> = driver
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, PlayerCommand::Seek(_)))
            .cloned()
            .collect();
        assert!(
            seeks.iter().any(|c| matches!(c, PlayerCommand::Seek(p) if p.is_zero())),
            "应 Seek(0) 回曲首: {seeks:?}"
        );
        // 队列/当前曲未变。
        assert_eq!(handle.state_rx.borrow().playback.current.as_ref().map(|t| t.id.as_ref()), Some("a"));
    }

    /// 里程碑 G：position ≤ 3s → 正常换上一首。
    #[tokio::test]
    async fn previous_switches_track_when_within_three_seconds() {
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![
            vec![TrackId::new("a")],
            vec![TrackId::new("b")],
        ]);
        let handle = PlaybackEngine::start(driver.clone(), Arc::new(resolver), Arc::new(|| true));
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        wait_idle().await;
        handle.cmd(Request::Command(PlayerCommand::Next)).await.unwrap();
        wait_idle().await;
        // 位置 ≤3s（默认 ~0）→ Previous 换回 a。
        let loads_before = driver.loads.lock().unwrap().len();
        handle.cmd(Request::Command(PlayerCommand::Previous)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(driver.loads.lock().unwrap().len() > loads_before, "≤3s 时 Previous 应换曲");
        assert_eq!(handle.state_rx.borrow().playback.current.as_ref().map(|t| t.id.as_ref()), Some("a"));
    }
```

（`wait_idle` helper 已存在；`PlayerCommand::Previous` 处理路径 = `navigate_prev`（handle_player_command 的 Previous 分支）✓；若 FakeDriver 初始 position 非 0，第二个测试先显式 `send_modify(position=0)`。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-daemon previous_restarts_track_when_past_three_seconds`
Expected: FAIL（>3s 时当前实现换曲——load 数增加断言失败）。

- [ ] **Step 3: 实现**

`crates/hmp-daemon/src/engine.rs` `navigate_prev` 开头：

```rust
    async fn navigate_prev(&mut self) {
        // 曲首语义（里程碑 G，审计第 6 步）：当前位置 > 3s → 只回曲首，
        // 不换曲（与会话记录/队列无交互；与 MPRIS Seek 行为一致）。
        if self.state_rx.borrow().position > std::time::Duration::from_secs(3) {
            self.driver.command(PlayerCommand::Seek(std::time::Duration::ZERO));
            return;
        }
        let saved = self.queue.save_state();
        …
    }
```

注意：`handle_player_command` 的 Previous 分支在 `navigate_prev().await` 后 `self.seq += 1; self.publish();`——曲首路径返回后仍会 seq+1 + publish（可接受：命令已处理，状态发布无害；会话不闭合 ✓）。

- [ ] **Step 4: 跑测试确认通过 + 全量**

Run: `cargo test -p hmp-daemon previous_ && cargo test -p hmp-daemon --lib && cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

- [ ] **Step 5: Commit**

```bash
git add crates/hmp-daemon/src/engine.rs
git commit -m "feat(engine): Previous restarts current track when >3s (seek 0, no track change)"
```

---

### Task 2: 输出设备选择（TDD）

**Files:**
- Modify: `crates/hmp-storage/src/config.rs`（`Config` 加 `[audio]` 段）
- Modify: `crates/hmp-daemon/src/serve.rs`（读配置传 `DaemonConfig`）
- Modify: `crates/hmp-daemon/src/daemon.rs`（`DaemonConfig` 文档注记——可选；**结构不变**）
- Test: `crates/hmp-storage/src/config.rs` tests + `crates/hmp-daemon/tests/e2e.rs`（配置驱动 sink 生效）

**Interfaces:**
- Consumes: 无。
- Produces:
  ```rust
  // config.rs
  /// 音频输出偏好（`[audio]` 段；`sink` = GStreamer sink 元素名，None = 系统默认）。
  #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
  pub struct AudioPref {
      #[serde(default)]
      pub sink: Option<String>,
  }
  // Config 加字段：
  #[serde(default)]
  pub audio: AudioPref,
  ```
- `serve.rs` `run_inner`：
  ```rust
  async fn run_inner(cfg: DaemonConfig) -> Result<(), Box<dyn std::error::Error>> {
      // 里程碑 G：输出设备来自 config.toml `[audio] sink`（无段 → 系统默认）。
      let audio = hmp_storage::Config::load().audio;
      let cfg = DaemonConfig {
          audio_sink: cfg.audio_sink.or(audio.sink),
          ..cfg
      };
      …
  }
  ```
  （保留 `DaemonConfig.audio_sink` 注入优先——测试/e2e 可用显式注入覆盖配置。）

- [ ] **Step 1: 写测试（先行）**

`crates/hmp-storage/src/config.rs` tests（现有 config 测试附近）追加：

```rust
    #[test]
    fn audio_sink_roundtrips_and_defaults() {
        let dir = std::env::temp_dir().join(format!("hmp-cfg-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
        // 隔离 XDG_CONFIG_HOME（仿现有测试的 TempGuard/EnvGuard 模式——以现有测试为准）。
        …
        // 写入含 [audio] 段的配置。
        std::fs::write(dir.join("hmp").join("config.toml"), "[audio]\nsink = \"fakesink\"\n").unwrap();
        let c = Config::load();
        assert_eq!(c.audio.sink.as_deref(), Some("fakesink"));
        // 无 [audio] 段 → None（旧配置兼容）。
        std::fs::write(dir.join("hmp").join("config.toml"), "[quality]\nmode = \"auto\"\n").unwrap();
        let c2 = Config::load();
        assert_eq!(c2.audio.sink, None);
        // 序列化往返。
        let c3 = Config { audio: AudioPref { sink: Some("pulsesink".into()) }, ..Default::default() };
        let text = toml::to_string(&c3).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.audio.sink.as_deref(), Some("pulsesink"));
    }
```

（`TEST_ENV_LOCK` 与 `TempGuard` 是 hmp-storage 测试基建——以现有 config.rs 测试的隔离模式为准。）

`crates/hmp-daemon/tests/e2e.rs`（现有 daemon 启动相关测试——若无 serve 启动测试，放 daemon_cli？**决策：e2e 加一个协议级测试**（不需真实音频）：`serve` 的 `Daemon::start` 用注入 cfg（`audio_sink: Some("fakesink")`）验证 GstDriver 创建成功 + 非法 sink 报错。若 e2e 无 Daemon::start 直接调用先例，改在 serve.rs 单元测试验证 `run_inner` 的配置合并逻辑（提取为纯函数）：

```rust
    #[test]
    fn audio_sink_config_merges_with_injection() {
        // 注入优先于配置；配置缺失 → None。
        let merged = merge_audio_sink(Some("injected"), Some("configed".into()));
        assert_eq!(merged, Some("injected"));
        let merged2 = merge_audio_sink(None, Some("configed".into()));
        assert_eq!(merged2, Some("configed"));
        let merged3 = merge_audio_sink(None, None);
        assert_eq!(merged3, None);
    }
```

**实现时以"最小可测面"为准**：`serve.rs` 提取 `fn merge_audio_sink(injected: Option<&str>, configured: Option<String>) -> Option<String>` 纯函数 + 单测；`run_inner` 调用它。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-storage audio_sink_roundtrips && cargo test -p hmp-daemon audio_sink_config_merges`
Expected: FAIL（编译失败：AudioPref/merge_audio_sink 不存在）。

- [ ] **Step 3: 实现**

1. `crates/hmp-storage/src/config.rs`：`AudioPref` + `Config.audio` 字段（serde default）。
2. `crates/hmp-daemon/src/serve.rs`：`merge_audio_sink` 纯函数 + `run_inner` 合并（`cfg.audio_sink.or(audio.sink)` 语义——注入优先）。

- [ ] **Step 4: 跑测试确认通过 + 全量**

Run: `cargo test -p hmp-storage && cargo test -p hmp-daemon && cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e`
Expected: 全绿。

- [ ] **Step 5: Commit**

```bash
git add crates/hmp-storage/src/config.rs crates/hmp-daemon/src/serve.rs
git commit -m "feat(storage,daemon): output device selection - config.toml [audio] sink wired to GstDriver"
```

---

### Task 3: 收尾验证

- [ ] **Step 1: 全量验证**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e && cargo test -p hmp-cli --test daemon_cli -- --ignored`
Expected: 全绿。

- [ ] **Step 2: 冒烟（可选）**

```bash
# config.toml 加 [audio] sink = "fakesink" → hmp serve --background → hmp status 正常
# （真实环境验证输出设备切换；无音频环境可跳过）
```

- [ ] **Step 3: 核对覆盖**

对照里程碑 G 第 1 批：Previous 曲首语义（✓ >3s Seek(0) 不换曲、≤3s 换曲、会话不闭合）；输出设备选择（✓ config.toml `[audio] sink` → GstDriver；注入优先；旧配置兼容）。G2 待办：gapless/preload（SourceResolver preload + 引擎下一首预解析）、ReplayGain（lofty RG 标签 → volume 补偿）。

- [ ] **Step 4: 报告**

报告：changed files、每任务验证、`cargo test --workspace` 总通过数、clippy/fmt 状态、未完成项（G2）。不要 git push（父会话统一推送并更新 roadmap）。
