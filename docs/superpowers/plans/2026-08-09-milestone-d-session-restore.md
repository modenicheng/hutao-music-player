# Session Restore Implementation Plan（里程碑 D）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** daemon 重启恢复 queue/current/position/volume/loop/shuffle（审计第 6 步；roadmap 里程碑 D）。恢复后**不自动播放**——队列与位置就绪，用户首次 Play 时从保存位置续播。

**Architecture:** 独立 JSON 状态文件（`$XDG_DATA_HOME/hmp/playback_state.json`，与 `library.sqlite3` 同级；不塞 SQLite——engine 的 `library: Option`，文件方案无库依赖、无迁移）。内容 = `QueueState`（已有，含 tracks/order/cursor/has_current/loop_mode/shuffle）+ volume + position_ms。engine 启动时读出并恢复（queue.restore_state + driver.set_volume），不自动播放；`publish()` 内做脏检查节流写盘（队列结构变更立即写、volume 变更写、position ≥5s 节流写、Paused/Stopped 立即写）；run 退出前最终写。`publish()` 由 `&self` 改为 `&mut self`（26 处调用点全在 `&mut self` 上下文；`saved` 镜像字段需要 &mut，`queue_tx` 的 `Cell<u64>` 可顺势改回普通字段）。

**Tech Stack:** Rust workspace（hmp-core / hmp-daemon）；serde/serde_json（已依赖）；tokio；FakeDriver 测试注入。

## Global Constraints

- 保持 `cargo test --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 全绿。
- `QueueState` 加 `Serialize/Deserialize` derive（`#[doc(hidden)]` 标注保留；hmp-core 已有 serde/serde_json 依赖）。
- 恢复路径不自动播放（决策点已定）；`Play` 命令首次成功装载且曲目 == 恢复的 current 时追加 `Seek(保存位置)`，随后清除恢复上下文。
- 写盘原子性：先写 `playback_state.json.tmp` 再 `rename`；写盘失败只 `tracing::warn`，绝不 panic/阻断播放。
- `start()`/`start_with_library`/`start_with_options` 签名变化：新增 `session_path: Option<PathBuf>`（`start()` 内部传 None）与 `persist_throttle: Duration`（`start_with_options` 注入用，默认 5s）。调用点共 8 处（daemon.rs:53、engine.rs:107/117/1071/1721/1772/2414/2455、server.rs:891）需适配。
- 每个任务独立 commit（`feat(daemon,…):` 前缀 + 中文要点）。

---

### Task 1: QueueState serde + SessionFile 类型与往返测试

**Files:**
- Modify: `crates/hmp-core/src/queue.rs`（QueueState derive）
- Modify: `crates/hmp-daemon/src/engine.rs`（SessionFile 类型 + 测试）
- Test: `crates/hmp-daemon/src/engine.rs` tests

**Interfaces:**
- Produces（Task 2 依赖）:
  ```rust
  // hmp-core queue.rs
  #[doc(hidden)]
  #[derive(Clone, Debug, Serialize, Deserialize)]
  pub struct QueueState { … }  // 字段不变

  // hmp-daemon engine.rs（模块级，非 pub）
  /// 会话持久化文件内容（`$XDG_DATA_HOME/hmp/playback_state.json`）。
  #[derive(Clone, Debug, Serialize, Deserialize)]
  struct SessionFile {
      /// 队列完整内部状态（restore_state 直接还原）。
      queue: hmp_core::queue::QueueState,
      /// 音量 0.0..=1.0。
      volume: f64,
      /// 当前曲播放位置（毫秒；has_current 时有效）。
      position_ms: u64,
  }
  ```

- [ ] **Step 1: 写往返测试（先行）**

`crates/hmp-daemon/src/engine.rs` tests 追加：

```rust
    #[test]
    fn session_file_roundtrips() {
        let mut q = hmp_core::QueueCore::new();
        q.append(vec![TrackId::new("qq:a"), TrackId::new("local:/x.mp3")]);
        q.set_current(1);
        q.set_loop_mode(LoopMode::List);
        q.set_shuffle(true);
        let f = SessionFile {
            queue: q.save_state(),
            volume: 0.42,
            position_ms: 12_345,
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: SessionFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.queue.tracks, f.queue.tracks);
        assert_eq!(back.queue.order, f.queue.order);
        assert_eq!(back.queue.cursor, 1);
        assert!(back.queue.has_current);
        assert_eq!(back.queue.loop_mode, LoopMode::List);
        assert!(back.queue.shuffle);
        assert_eq!(back.volume, 0.42);
        assert_eq!(back.position_ms, 12_345);
    }

    #[test]
    fn session_file_missing_is_none() {
        // 无状态文件 → 恢复为 None（首次启动路径）。
        assert!(read_session_file("/nonexistent/hmp-test/no-such.json").unwrap().is_none());
    }

    #[test]
    fn session_file_corrupt_is_none() {
        // 损坏文件不 panic，视为无会话。
        let dir = std::env::temp_dir().join(format!("hmp-session-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("playback_state.json");
        std::fs::write(&p, "{ not valid json !!!").unwrap();
        let r = read_session_file(&p);
        assert!(r.is_ok());
        assert!(r.unwrap().is_none());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-daemon session_file_`
Expected: FAIL（编译失败：`SessionFile`/`read_session_file` 不存在；QueueState 无 serde——若 serde 缺失导致编译错误也符合预期）。

- [ ] **Step 3: 实现**

`crates/hmp-core/src/queue.rs`：`#[derive(Clone, Debug)]` → `#[derive(Clone, Debug, Serialize, Deserialize)]`（确认 `use serde::{Deserialize, Serialize};` 已存在，缺则加）。

`crates/hmp-daemon/src/engine.rs` 模块级（`SessionFile` 定义如上）+ 读函数：

```rust
/// 读取会话文件（不存在/损坏 → None，不报错）。
fn read_session_file(path: &std::path::Path) -> std::io::Result<Option<SessionFile>> {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(f) => Ok(Some(f)),
            Err(e) => {
                tracing::warn!(%e, "会话文件损坏，忽略");
                Ok(None)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// 原子写会话文件（tmp + rename）。
fn write_session_file(path: &std::path::Path, f: &SessionFile) -> std::io::Result<()> {
    let json = serde_json::to_vec(f).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p hmp-daemon session_file_`
Expected: 3 个 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/hmp-core/src/queue.rs crates/hmp-daemon/src/engine.rs
git commit -m "feat(core,daemon): SessionFile serde type - QueueState serializable + atomic read/write"
```

---

### Task 2: engine 恢复 + 节流持久化（TDD）

**Files:**
- Modify: `crates/hmp-daemon/src/engine.rs`（构造参数、恢复、publish 脏检查、Play 恢复 seek、退出最终写）
- Modify: `crates/hmp-daemon/src/daemon.rs`（传 session_path）
- Modify: `crates/hmp-daemon/src/server.rs`（start_with_library 调用适配）
- Test: `crates/hmp-daemon/src/engine.rs` tests

**Interfaces:**
- Consumes: Task 1 的 `SessionFile`/`read_session_file`/`write_session_file`；`QueueCore::restore_state`。
- Produces（Task 3 依赖）:
  - `start_with_library(driver, resolver, credential_ok, library, session_path: Option<PathBuf>)`
  - `start_with_options(driver, resolver, credential_ok, library, load_timeout, session_path: Option<PathBuf>, persist_throttle: Duration)`
  - engine 启动后（run 开始前同步完成）恢复；`publish(&mut self)` 节流写盘。

- [ ] **Step 1: 写测试（先行）**

`crates/hmp-daemon/src/engine.rs` tests 追加（helper 复用现有 FakeDriver/FakeResolver——注意现有 helper 是否可从 tests 模块访问 `SessionFile`/`write_session_file`：同文件模块内 ✓）：

```rust
    /// 会话恢复集成：重启后队列/音量/位置恢复，不自动播放，Play 后续播。
    #[tokio::test]
    async fn session_restores_after_restart() {
        let dir = std::env::temp_dir().join(format!("hmp-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sp = dir.join("playback_state.json");
        // 第一代引擎：构造队列、设音量、推进位置后"退出"（写盘发生在前台命令+节流）。
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![
            vec![TrackId::new("qq:r1")],
        ]);
        let h1 = PlaybackEngine::start_with_options(
            driver.clone(),
            Arc::new(resolver),
            Arc::new(|| true),
            None,
            std::time::Duration::from_secs(5),
            Some(sp.clone()),
            std::time::Duration::from_millis(0), // 节流 0 → 每次 publish 都写
        );
        h1.cmd(Request::Command(PlayerCommand::SetVolume(0.37))).await.unwrap();
        h1.cmd(Request::Play(PlayRequest::Track(TrackId::new("qq:r1")))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        h1.cmd(Request::Command(PlayerCommand::SetShuffle(true))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(sp.exists(), "会话文件应已写入");
        // 第二代引擎：同路径恢复。
        let (driver2, _, _) = FakeDriver::new();
        let resolver2 = FakeResolver::new(vec![vec![TrackId::new("qq:r1")]]);
        let h2 = PlaybackEngine::start_with_options(
            driver2.clone(),
            Arc::new(resolver2),
            Arc::new(|| true),
            None,
            std::time::Duration::from_secs(5),
            Some(sp.clone()),
            std::time::Duration::from_secs(5),
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let st = h2.state_rx.borrow().clone();
        // 恢复不自动播放。
        assert_ne!(st.playback.status, hmp_core::PlaybackStatus::Playing);
        // 队列已恢复（含洗牌开关——QueueState 整体还原）。
        assert_eq!(h2.queue_rx.borrow().tracks, vec![TrackId::new("qq:r1")]);
        assert!(h2.queue_rx.borrow().shuffle);
        // 音量已恢复（start 时 driver.set_volume 已调用）。
        assert_eq!(st.playback.volume, 0.37);
        // Play 后从保存位置续播（driver 收到 Seek）。
        h2.cmd(Request::Play(PlayRequest::Track(TrackId::new("qq:r1")))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let seeks: Vec<PlayerCommand> = driver2
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, PlayerCommand::Seek(_)))
            .cloned()
            .collect();
        assert!(!seeks.is_empty(), "恢复后首次 Play 应发出 Seek 续播");
    }

    /// 节流：位置推进不触发频繁写盘（throttle=5s 时 100ms 内只写有限次数）。
    #[tokio::test]
    async fn position_persist_is_throttled() {
        let dir = std::env::temp_dir().join(format!("hmp-session-throttle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sp = dir.join("playback_state.json");
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("qq:t1")]]);
        let h = PlaybackEngine::start_with_options(
            driver.clone(),
            Arc::new(resolver),
            Arc::new(|| true),
            None,
            std::time::Duration::from_secs(5),
            Some(sp.clone()),
            std::time::Duration::from_secs(5), // 长节流
        );
        h.cmd(Request::Play(PlayRequest::Track(TrackId::new("qq:t1")))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let mtime1 = std::fs::metadata(&sp).unwrap().modified().unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let mtime2 = std::fs::metadata(&sp).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "节流期内 position 推进不应触发写盘");
    }

    /// 写盘失败不 panic、不阻断播放。
    #[tokio::test]
    async fn session_persist_failure_is_swallowed() {
        let sp = std::path::PathBuf::from("/nonexistent-dir-hmp/playback_state.json");
        let (driver, _, _) = FakeDriver::new();
        let resolver = FakeResolver::new(vec![vec![TrackId::new("qq:f1")]]);
        let h = PlaybackEngine::start_with_options(
            driver.clone(),
            Arc::new(resolver),
            Arc::new(|| true),
            None,
            std::time::Duration::from_secs(5),
            Some(sp),
            std::time::Duration::from_millis(0),
        );
        h.cmd(Request::Play(PlayRequest::Track(TrackId::new("qq:f1")))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let st = h.state_rx.borrow().clone();
        assert_eq!(st.playback.status, hmp_core::PlaybackStatus::Playing);
    }
```

注意：`FakeDriver::set_volume`（874 行）为空实现——恢复音量经 `driver.set_volume` 调用（可加记录或直接依赖 playback.volume 断言——FakeDriver 的 state 里 volume 由谁更新？检查 FakeDriver 是否维护 volume 状态：若 `SetVolume` 命令不更新 FakeDriver 内部 state，则 `st.playback.volume` 断言可能不成立。**以实际为准**：若 FakeDriver 不反映 volume，改断言为检查 driver.commands 里有 `SetVolume(0.37)`）。`PlayRequest`/`PlayerCommand::SetVolume` 的 CLI 命令路径确认（engine 测试中现有用法，如 1267 行 `PlayerCommand::SetShuffle(true)` ✓）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-daemon session_restores_after_restart`
Expected: FAIL（编译失败：签名不匹配）。

- [ ] **Step 3: 实现 engine 集成**

`crates/hmp-daemon/src/engine.rs`：

**新字段**（PlaybackEngine 结构）：

```rust
    /// 会话持久化路径（None = 不持久化）。
    session_path: Option<std::path::PathBuf>,
    /// 位置写盘节流。
    persist_throttle: std::time::Duration,
    /// 上次写盘时的内存镜像（脏检查）。
    saved: SessionMirror,
    /// 恢复的会话（Play 时应用 seek；应用后清除）。
    restored: Option<RestoredSession>,
```

```rust
/// 持久化镜像（脏检查基准）。
#[derive(Default)]
struct SessionMirror {
    queue_rev: u64,
    volume: f64,
    /// (曲目 id, 位置 ms) 上次写入值。
    position: Option<(TrackId, u64)>,
    /// 上次写盘时刻（位置节流用）。
    last_write: Option<std::time::Instant>,
    /// 当前是否处于非播放状态（Paused/Stopped → 立即写）。
    playing: bool,
}

/// 启动时恢复的会话上下文。
struct RestoredSession {
    /// 恢复的当前曲（Play 时若装载同曲则 Seek 续播）。
    current: TrackId,
    position_ms: u64,
}
```

`start_with_options` 签名与实现（第 6、7 参数）：`session_path: Option<PathBuf>, persist_throttle: Duration`；`start_with_library` 加 `session_path`（透传）；`start()` 内部传 None（经 start_with_library）。

**启动恢复**（start_with_options 内，engine 构造前——同步读，避免 run 竞态）：

```rust
        // 启动恢复：队列/音量/位置（不自动播放；Play 时续播，里程碑 D）。
        let (restored_queue, restored_volume, restored_session) = match &session_path {
            Some(p) => match read_session_file(p) {
                Ok(Some(f)) => (Some(f.queue), Some(f.volume), Some(f)),
                Ok(None) => (None, None, None),
                Err(e) => {
                    tracing::warn!(%e, "读取会话文件失败");
                    (None, None, None)
                }
            },
            None => (None, None, None),
        };
        let mut queue = hmp_core::QueueCore::new();
        if let Some(q) = restored_queue {
            queue.restore_state(q);
        }
        if let Some(v) = restored_volume {
            driver.set_volume(v);
        }
        let restored = restored_session.map(|f| RestoredSession {
            current: f.queue.tracks[f.queue.order[f.queue.cursor]].clone(),
            position_ms: f.position_ms,
        });
        // 仅在 has_current 且存在当前曲时保留 restored：
        let restored = match (&restored, queue.current_track()) {
            (Some(r), Some(cur)) if *cur == r.current => restored,
            _ => None,
        };
```

注意 `QueueCore::current_track()` 是否存在——确认方法名（`current_idx()` 233 行存在；current_track 可能没有——用 `self.queue.current_idx().map(|i| …)` 或 tracks[order[cursor]] 帮助方法；**以实际为准**，若无则：`queue.snapshot().current`（会克隆，仅启动一次可接受）或直接解 QueueState）。

engine 构造：`session_path, persist_throttle, saved: SessionMirror::default(), restored`；`last_queue_rev: Cell` 改为普通 `u64` 字段（publish 改 &mut self 后不再需要 Cell——**决策：保留 Cell 减少改动面，还是改普通字段？** publish(&mut self) 后普通字段更干净——改。若 Cell 在别处用（engine.rs 搜索 last_queue_rev 仅 publish 用）则改普通字段）。

**publish() 改 &mut self + 尾部持久化**：

```rust
    fn publish(&mut self) {
        let caps = …;
        let state = DaemonState { … };
        let _ = self.state_tx.send(state);
        if self.last_queue_rev != self.queue.revision() {
            self.last_queue_rev = self.queue.revision();
            let _ = self.queue_tx.send(self.queue.snapshot());
        }
        let _ = self.caps_tx.send(caps);
        self.persist_session();
    }

    /// 脏检查 + 节流写盘（写失败仅告警，不阻断播放）。
    fn persist_session(&mut self) {
        let Some(path) = self.session_path.clone() else { return };
        let playback = self.state_rx.borrow().clone();
        let rev = self.queue.revision();
        let volume = playback.volume;
        let position = self
            .queue
            .current_idx()
            .and_then(|i| self.queue.track_at(i))
            .map(|id| (id.clone(), playback.position.as_millis() as u64));
        let playing = playback.status == PlaybackStatus::Playing;
        let mut dirty = rev != self.saved.queue_rev || (volume - self.saved.volume).abs() > 1e-9;
        if position != self.saved.position {
            let now = std::time::Instant::now();
            let throttled = matches!(self.saved.last_write, Some(t) if now.duration_since(t) < self.persist_throttle);
            let must_flush = !playing || !throttled;
            dirty |= must_flush && position.is_some();
        }
        if playing != self.saved.playing {
            dirty = true; // 播放状态翻转（开始/暂停/停止）立即写
            self.saved.playing = playing;
        }
        if !dirty {
            return;
        }
        let f = SessionFile {
            queue: self.queue.save_state(),
            volume,
            position_ms: position.map(|(_, ms)| ms).unwrap_or(0),
        };
        match write_session_file(&path, &f) {
            Ok(()) => {
                self.saved.queue_rev = rev;
                self.saved.volume = volume;
                self.saved.position = position;
                self.saved.last_write = Some(std::time::Instant::now());
            }
            Err(e) => tracing::warn!(%e, "写入会话文件失败"),
        }
    }
```

`track_at(i)` 方法确认存在（QueueCore 有 `track_at`？——grep 确认；若无用 `snapshot()` 或 `tracks.get(i)` 的公开等价物；**以实际为准**，最稳：`let snap = self.queue.snapshot();` 在 persist_session 内用（仅脏时才 clone——先算 dirty 再 clone 更好；若 track_at 不存在，把 snapshot 放 dirty 检查后）。

**Play 续播**（handle_player_command 的 Play 分支——找到 `PlayerCommand::Play`/`PlayRequest` 处理处，装载成功后）：

```rust
            // 恢复会话续播：首次 Play 且装载曲目 == 恢复的 current → Seek 到保存位置。
            if let Some(r) = self.restored.take() {
                if *id == r.current && playback_result.is_ok() {
                    self.driver.command(PlayerCommand::Seek(std::time::Duration::from_millis(r.position_ms)));
                }
            }
```

（位置：Play 分支内 load_and_play 成功之后、publish 之前；以现有 Play 分支结构为准最小插入。）

**退出最终写**：run() 的 loop 结束后（tokio::spawn 闭包 `engine.run().await` 之后）——spawn 闭包里 engine 已被 run(&mut self) 借用后释放，闭包内 `engine.persist_session()` 仍可调（run 返回后 &mut 可用）。在 spawn 闭包加：

```rust
            engine.run().await;
            engine.persist_session(); // 退出前最终写（保真位置/队列）。
            let _ = engine.term_tx.send(true);
```

注意：Paused/Stopped 已在 publish 即时写；退出时若 Playing 也最终写一次。

**volume 直通问题**：`SetVolume` 直通 driver（379 行 `other =>`）→ engine 的 publish 被谁触发？driver state 变化（volume 变了）→ run() 的 `state_rx.changed()` 分支 → publish ✓（volume 变化会触发 driver watch 更新 → publish → 脏检查 volume 不同 → 写盘 ✓）。

- [ ] **Step 4: 适配现有调用点**

- `engine.rs:107` `start()`：`Self::start_with_library(driver, resolver, credential_ok, None, None)`（加 None）
- `engine.rs:117` `start_with_library`：透传 `session_path` → `start_with_options(..., None, load_timeout, session_path, Duration::from_secs(5))`——**注意**：start_with_library 调 start_with_options 时 persist_throttle 用默认 5s。
- `engine.rs:1071/1721/1772/2414/2455`、`server.rs:891`：加 `None`（或测试注入临时路径）；1721/1772 等 start_with_options 调用加 `None, Duration::from_secs(5)`。
- `daemon.rs:53`（Task 3 做，本任务先保持编译——不行，签名变了必须同步改：传 `Some(hmp_storage::data_dir().join("playback_state.json"))`）。

- [ ] **Step 5: 跑测试**

Run: `cargo test -p hmp-daemon session_ && cargo test -p hmp-daemon --lib`
Expected: 3 个新测试 PASS + 现有全绿（FakeDriver volume 行为若与断言不符，按 Step 1 注记调整断言方式）。

- [ ] **Step 6: 全量 + Commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

```bash
git add crates/hmp-daemon/src/engine.rs crates/hmp-daemon/src/daemon.rs crates/hmp-daemon/src/server.rs
git commit -m "feat(daemon): session restore - queue/volume/position persisted, resumed on first Play, no autoplay"
```

---

### Task 3: 收尾验证

- [ ] **Step 1: 全量验证**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e && cargo test -p hmp-cli --test daemon_cli -- --ignored`
Expected: 全绿。

- [ ] **Step 2: 核对覆盖**

对照审计第 6 步：队列恢复（✓ restore_state 整体还原含 order/cursor/loop/shuffle）、音量（✓ driver.set_volume）、位置（✓ 恢复上下文 + 首次 Play Seek）、不自动播放（✓ 决策已实现）、节流（✓ 5s）、原子写（✓ tmp+rename）、写失败容忍（✓ warn 不 panic）。

- [ ] **Step 3: 报告**

报告：changed files、每任务验证、`cargo test --workspace` 总通过数、clippy/fmt 状态、未完成项。不要 git push（父会话统一推送并更新 roadmap）。
