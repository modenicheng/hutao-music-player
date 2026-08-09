# SQLite/Outbox Atomic Writes Implementation Plan（里程碑 B）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 消除"本地修改已落库、远端同步意图丢失"窗口：owned 歌单加曲/删曲、subscribed 取消收藏、owned 删除的"本地写 + outbox 入队"全部合并为单事务（审计 P1 #6；roadmap 里程碑 B）。

**Architecture:** hmp-storage 新增 4 个组合方法（`execute_batch("BEGIN")` → 内部调用现有无事务方法 → `COMMIT`，失败 `ROLLBACK`），server.rs 的 4 个 PlaylistWrite 分支改调组合方法。内部方法（`add_playlist_track`/`remove_playlist_track`/`delete_playlist`/`enqueue_playlist_op`/`set_relation`/`mark_playlist_pending`）均已确认内部无 `BEGIN`，无嵌套事务冲突；`upsert_tracks_batch` 有事务但组合路径不经过它。

**Tech Stack:** Rust workspace（hmp-storage / hmp-daemon）；rusqlite；TDD（失败注入用 DROP TABLE 制造第二步失败）。

## Global Constraints

- 保持 `cargo test --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 全绿。
- 不改动任何现有方法签名（组合方法为新增）；不改 IPC 协议。
- 事务模式与 `record_play_end`（db.rs:258-277）一致：`BEGIN` → 闭包执行 → `COMMIT`/`ROLLBACK`。
- 行为语义不变：AddTrack 去重逻辑、RemoveTrack 的 local: 跳过、subscribed 删除的 unfav 触发（`trigger_sync`）保持。
- 每个任务独立 commit（`feat(storage,…):` 前缀 + 中文要点）。

---

### Task 1: storage 组合方法（TDD）

**Files:**
- Modify: `crates/hmp-storage/src/db.rs`（`enqueue_playlist_op` 之后新增 4 方法；tests 模块追加）
- Test: `crates/hmp-storage/src/db.rs` tests

**Interfaces:**
- Consumes: 现有 `add_playlist_track(&mut self, playlist_id: i64, source: &'static str, source_key: &str, title: &str) -> rusqlite::Result<()>`（db.rs:1082）、`remove_playlist_track(&mut self, playlist_id: i64, position: i64) -> rusqlite::Result<()>`（1129）、`delete_playlist(&mut self, id: i64) -> rusqlite::Result<()>`（1022，内部级联清 playlist_ops/playlist_tracks）、`enqueue_playlist_op(&mut self, playlist_id: i64, op: &str, song_key: Option<&str>, song_id: Option<i64>) -> rusqlite::Result<()>`（936）、`set_relation(&mut self, entity_type: &str, provider: &str, entity_key: &str, relation: &str, desired: bool) -> rusqlite::Result<()>`（537）、`mark_playlist_pending(&mut self, id: i64) -> rusqlite::Result<()>`（826）。
- Produces（Task 2 依赖）:
  ```rust
  pub fn add_owned_track_with_op(
      &mut self,
      playlist_id: i64,
      source: &'static str,
      source_key: &str,
      title: &str,
      song_id: Option<i64>,
  ) -> rusqlite::Result<()>
  // 单事务：add_playlist_track + enqueue_playlist_op("add", Some(source_key), song_id)

  pub fn remove_owned_track_with_op(
      &mut self,
      playlist_id: i64,
      position: i64,
      song_key: &str,
      song_id: Option<i64>,
  ) -> rusqlite::Result<()>
  // 单事务：remove_playlist_track + enqueue_playlist_op("del", Some(song_key), song_id)

  pub fn unfavorite_playlist(
      &mut self,
      playlist_id: i64,
      remote_id: Option<&str>,
  ) -> rusqlite::Result<()>
  // 单事务：delete_playlist + （remote_id 存在时）set_relation("playlist","qq",remote_id,"subscribed",false)

  pub fn mark_pending_with_delete_op(&mut self, playlist_id: i64) -> rusqlite::Result<()>
  // 单事务：mark_playlist_pending + enqueue_playlist_op("delete_playlist", None, None)
  ```

- [ ] **Step 1: 写失败注入测试（先行）**

`crates/hmp-storage/src/db.rs` tests 模块追加（仿现有 playlist 测试区，约 1199-1240 行附近；`TrackRow` 构造对照现有测试）：

```rust
    #[test]
    fn add_owned_track_with_op_rolls_back_on_op_failure() {
        // 第二条语句（enqueue op）失败 → 第一条（本地关联）必须整体回滚。
        let mut db = LibraryDb::open_in_memory().unwrap();
        let pid = db.create_playlist("p").unwrap();
        db.upsert_track(&TrackRow {
            source: "qq".into(),
            source_key: "mid-x".into(),
            title: "x".into(),
            album: None,
            artist: None,
            duration_ms: None,
            cover_uri: None,
            qq_song_id: None,
        })
        .unwrap();
        // 破坏 outbox 表制造 enqueue 失败（独立内存库，不影响其他测试）。
        db.conn.execute_batch("DROP TABLE playlist_ops").unwrap();
        let r = db.add_owned_track_with_op(pid, "qq", "mid-x", "x", Some(1));
        assert!(r.is_err(), "op 入队失败时组合方法必须报错");
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM playlist_tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "本地曲目关联不得残留（整体回滚）");
        let t: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(t, 0, "upsert 的 tracks 行也不得残留");
    }

    #[test]
    fn add_owned_track_with_op_commits_atomically() {
        // 正常路径：本地行 + outbox 行都落库。
        let mut db = LibraryDb::open_in_memory().unwrap();
        let pid = db.create_playlist("p").unwrap();
        db.add_owned_track_with_op(pid, "qq", "mid-x", "x", Some(1))
            .unwrap();
        let pt: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM playlist_tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pt, 1);
        let ops = db.playlist_ops_pending().unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].op, "add");
        assert_eq!(ops[0].song_key.as_deref(), Some("mid-x"));
        assert_eq!(ops[0].song_id, Some(1));
    }

    #[test]
    fn unfavorite_playlist_rolls_back_on_relation_failure() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let pid = db.create_playlist("p").unwrap();
        // 破坏 relations 表制造 set_relation 失败（第二步）。
        db.conn.execute_batch("DROP TABLE relations").unwrap();
        let r = db.unfavorite_playlist(pid, Some("disstid-1"));
        assert!(r.is_err());
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM playlists", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "歌单行必须保留（整体回滚）");
    }

    #[test]
    fn unfavorite_playlist_commits_atomically() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let pid = db.create_playlist("p").unwrap();
        db.set_relation("playlist", "qq", "disstid-1", "subscribed", true)
            .unwrap();
        db.unfavorite_playlist(pid, Some("disstid-1")).unwrap();
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM playlists", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "歌单已删除");
        let rel = db
            .relation_desired("playlist", "qq", "disstid-1", "subscribed")
            .unwrap();
        assert_eq!(rel, Some(false), "取消收藏已入 relations outbox");
    }

    #[test]
    fn mark_pending_with_delete_op_rolls_back_on_op_failure() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let pid = db.create_playlist("p").unwrap();
        db.conn.execute_batch("DROP TABLE playlist_ops").unwrap();
        let r = db.mark_pending_with_delete_op(pid);
        assert!(r.is_err());
        let st: String = db
            .conn
            .query_row("SELECT sync_state FROM playlists WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(st, "synced", "pending 标记不得残留（整体回滚）");
    }
```

注意：若测试模块无法直接访问 `db.conn`（私有字段），检查现有测试是否已直接使用（现有 `play_start_returns_id_and_end_closes_by_id` 已用 `db.conn`——确认后沿用）。`PlaylistOpRow` 字段名以 `playlist_ops_pending` 查询映射为准（`op`/`song_key`/`song_id`）。`create_playlist` 返回 id（现有测试确认）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-storage add_owned_track_with_op_rolls_back_on_op_failure`
Expected: FAIL（编译失败：方法不存在）。

- [ ] **Step 3: 实现 4 个组合方法**

`crates/hmp-storage/src/db.rs` 中 `enqueue_playlist_op` 之后新增（事务模式照抄 `record_play_end`）：

```rust
    /// owned 歌单加曲 + outbox 入队（单事务）：任一失败整体回滚，
    /// 不留"本地已改、远端意图丢失"窗口。
    pub fn add_owned_track_with_op(
        &mut self,
        playlist_id: i64,
        source: &'static str,
        source_key: &str,
        title: &str,
        song_id: Option<i64>,
    ) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            self.add_playlist_track(playlist_id, source, source_key, title)?;
            self.enqueue_playlist_op(playlist_id, "add", Some(source_key), song_id)?;
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                self.conn.execute_batch("ROLLBACK").ok();
                return Err(e);
            }
        }
        Ok(())
    }

    /// owned 歌单删曲 + outbox 入队（单事务）。调用方负责确认 song_key
    /// 非 local:（远端无对应物时不入队）。
    pub fn remove_owned_track_with_op(
        &mut self,
        playlist_id: i64,
        position: i64,
        song_key: &str,
        song_id: Option<i64>,
    ) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            self.remove_playlist_track(playlist_id, position)?;
            self.enqueue_playlist_op(playlist_id, "del", Some(song_key), song_id)?;
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                self.conn.execute_batch("ROLLBACK").ok();
                return Err(e);
            }
        }
        Ok(())
    }

    /// 取消收藏（subscribed 歌单删除）：删本地歌单 + relations unfav（单事务）。
    /// remote_id 为 None（本地歌单/无远端身份）时只删本地行。
    pub fn unfavorite_playlist(
        &mut self,
        playlist_id: i64,
        remote_id: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            self.delete_playlist(playlist_id)?;
            if let Some(rid) = remote_id {
                self.set_relation("playlist", "qq", rid, "subscribed", false)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                self.conn.execute_batch("ROLLBACK").ok();
                return Err(e);
            }
        }
        Ok(())
    }

    /// owned 歌单删除：pending 标记 + delete_playlist op 入队（单事务）。
    pub fn mark_pending_with_delete_op(&mut self, playlist_id: i64) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            self.mark_playlist_pending(playlist_id)?;
            self.enqueue_playlist_op(playlist_id, "delete_playlist", None, None)?;
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                self.conn.execute_batch("ROLLBACK").ok();
                return Err(e);
            }
        }
        Ok(())
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p hmp-storage add_owned_track_with_op_rolls_back_on_op_failure add_owned_track_with_op_commits_atomically unfavorite_playlist_rolls_back_on_relation_failure unfavorite_playlist_commits_atomically mark_pending_with_delete_op_rolls_back_on_op_failure`
Expected: 5 个 PASS。

- [ ] **Step 5: 全量 + Commit**

Run: `cargo test -p hmp-storage && cargo clippy -p hmp-storage --all-targets -- -D warnings`
Expected: 全绿。

```bash
git add crates/hmp-storage/src/db.rs
git commit -m "feat(storage): atomic playlist+outbox composite methods - add/remove owned track, unfavorite, mark pending delete"
```

---

### Task 2: server.rs 调用点替换

**Files:**
- Modify: `crates/hmp-daemon/src/server.rs`（PlaylistWrite 处理，约 236-300 行）
- Test: `crates/hmp-daemon/src/server.rs` tests（现有 playlist 写测试适配/补充）

**Interfaces:**
- Consumes: Task 1 的 4 个组合方法。
- Produces: `PlaylistWrite` 的 owned/subscribed 分支全部单事务；`trigger_sync` 语义不变。

- [ ] **Step 1: 替换 4 处**

`crates/hmp-daemon/src/server.rs`：

Delete owned 分支：

```rust
                            Ok(Some(r)) if r == "owned" => {
                                // 行保留到远端删除成功（op 驱动）；pending + op 单事务。
                                lib.mark_pending_with_delete_op(id)?;
                                trigger_sync = true;
                                Ok(None)
                            }
```

Delete subscribed 分支：

```rust
                            Ok(Some(r)) if r == "subscribed" => {
                                // 取消收藏：删本地行 + relations outbox 单事务
                                // （unfav 同步成功前 reconcile 不覆盖 pending，不会复活）。
                                let remote = lib.playlist_remote_id(id)?;
                                lib.unfavorite_playlist(id, remote.as_deref())?;
                                if remote.is_some() {
                                    trigger_sync = true;
                                }
                                Ok(None)
                            }
```

AddTrack owned 分支（保留前面的 subscribed/local 拒绝逻辑，替换最后两行）：

```rust
                            lib.add_playlist_track(id, source_static, &key, &title)?;
                            if rel == "owned" {
                                let song_id = lib.qq_song_id("qq", &key)?;
                                lib.enqueue_playlist_op(id, "add", Some(&key), song_id)?;
                                trigger_sync = true;
                            }
```

改为：

```rust
                            if rel == "owned" {
                                let song_id = lib.qq_song_id("qq", &key)?;
                                lib.add_owned_track_with_op(id, source_static, &key, &title, song_id)?;
                                trigger_sync = true;
                            } else {
                                lib.add_playlist_track(id, source_static, &key, &title)?;
                            }
```

RemoveTrack owned 分支（保留 subscribed 拒绝与 local: 跳过，替换删行+入队为组合调用）：

```rust
                            let song_key = lib.track_key_at(id, position)?;
                            lib.remove_playlist_track(id, position)?;
                            if rel == "owned" {
                                if let Some(key) = song_key {
                                    // local 曲目在远端无对应物：只删本地行，不入 outbox
                                    // （否则 song_id 恒 None → 永久 error 重试）。
                                    if !key.starts_with("local:") {
                                        let song_id = lib.qq_song_id("qq", &key)?;
                                        lib.enqueue_playlist_op(id, "del", Some(&key), song_id)?;
                                        trigger_sync = true;
                                    }
                                }
                            }
```

改为：

```rust
                            let song_key = lib.track_key_at(id, position)?;
                            if rel == "owned" {
                                if let Some(key) = song_key {
                                    // local 曲目在远端无对应物：只删本地行，不入 outbox
                                    // （否则 song_id 恒 None → 永久 error 重试）。
                                    if !key.starts_with("local:") {
                                        let song_id = lib.qq_song_id("qq", &key)?;
                                        lib.remove_owned_track_with_op(id, position, &key, song_id)?;
                                        trigger_sync = true;
                                        Ok(None)
                                    } else {
                                        lib.remove_playlist_track(id, position)?;
                                        Ok(None)
                                    }
                                } else {
                                    lib.remove_playlist_track(id, position)?;
                                    Ok(None)
                                }
                            } else {
                                lib.remove_playlist_track(id, position)?;
                                Ok(None)
                            }
```

（保持 match 臂返回 `Result<Option<i64>>` 的形状——`Ok(None)` 与现有 `Ok(_) =>` 结构一致；注意 RemoveTrack 臂原代码是表达式序列，改为显式返回时确认与相邻臂类型一致。）

- [ ] **Step 2: 适配/补充 server 测试**

`crates/hmp-daemon/src/server.rs` tests：现有 playlist 写测试（AddTrack owned 产生 op、Delete subscribed 产生 unfav 等）应全部保持通过（组合方法行为等价）。新增一条单事务断言（直接构造 server 场景较难——若 tests 已有通过 handle 发 PlaylistWrite 的模式则补；若无则依赖 storage 层 Task 1 测试覆盖，server 层仅验证行为等价）。

Run: `cargo test -p hmp-daemon --lib playlist`
Expected: 现有 playlist 相关测试全绿。

- [ ] **Step 3: 全量 + Commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

```bash
git add crates/hmp-daemon/src/server.rs
git commit -m "feat(daemon): playlist writes use atomic storage composites - no local/outbox split window"
```

---

### Task 3: 收尾验证

- [ ] **Step 1: 全量验证**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e && cargo test -p hmp-cli --test daemon_cli -- --ignored`
Expected: 全绿。

- [ ] **Step 2: 核对覆盖**

对照审计 P1 #6：owned AddTrack（✓ Task 1/2）、RemoveTrack（✓）、subscribed Delete（✓）、owned Delete pending+op（✓，原窗口 mark_playlist_pending→enqueue 两步）。`Favorite` 命令本身已单事务（set_relation），无需改。

- [ ] **Step 3: 报告**

报告：changed files、每任务验证、`cargo test --workspace` 总通过数、clippy/fmt 状态、未完成项。不要 git push（父会话统一推送并更新 roadmap）。
