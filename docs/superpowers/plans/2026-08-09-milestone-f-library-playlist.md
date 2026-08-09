# LibraryPlaylist Playback Source Implementation Plan（里程碑 F）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 本地 SQLite 歌单成为播放源（审计 P1 #8；"能管理，不能播放"）。`hmp play playlist:local:<id>` 从 `playlist_tracks JOIN tracks` 直读曲目列表播放（无需网络），支持 QQ 曲目与本地曲目混排（`resolve_track` 经 Composite 按 provider 分发，已有机制）。

**决策定案（本计划权威）：**
1. **显式拆分变体**：`PlayRequest::LibraryPlaylist(i64)` 新变体（roadmap 决策点；避免与 QQ songlist 数字 id 空间歧义）。`playlist:<id>` 保留 QQ 语义不动。
2. **CLI 语法**：`playlist:local:<id>`（parse_source 前缀识别；`hmp playnext` 同理）。
3. **不做去重语义变更**：`add_playlist_track` 幂等去重保持（里程碑 B 组合方法建立其上）；roadmap 的"occurrence id 支持重复曲目"记为已知限制（低优先，不阻塞播放）。
4. **协议变更**：`PlayRequest` 枚举加变体（serde derive 自动）——旧客户端反序列化新请求失败属预期（同仓同步发布，C 里程碑先例）。

**Architecture:** hmp-core `PlayRequest::LibraryPlaylist(i64)`；`CompositeSourceResolver` 分发 `LibraryPlaylist` → `LocalSourceResolver`（查 `playlist_tracks JOIN tracks` 完整列 → `TrackStub` 列表；空歌单/不存在 → `PlaylistNotFound`）；CLI `parse_source` 加 `playlist:local:` 前缀。engine 的 `play_source` 走统一 `resolve_source_ids` 入口，无需改动（除编译要求的 match 补全）。

**Tech Stack:** Rust workspace（hmp-core / hmp-daemon / hmp-cli）；现有 storage 查询。

## Global Constraints

- 保持 `cargo test --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 全绿。
- `PlayRequest::Playlist`/`QqSourceResolver` 的 QQ 分支**不动**（行为不变）。
- 本地歌单解析：曲目按 `position` 排序；TrackStub.id = `source_key`（`qq:mid` 或 `local:<path>` 原样）；artist/album/duration 从 tracks 表带出（缺失为 None 不阻断）。
- 每个 Task 独立 commit（`feat(core,…)` 前缀 + 中文要点）。

---

### Task 1: PlayRequest::LibraryPlaylist 变体 + CLI 语法（TDD）

**Files:**
- Modify: `crates/hmp-core/src/ipc.rs`（枚举 + roundtrip 测试）
- Modify: `crates/hmp-cli/src/commands.rs`（parse_source + 测试）
- Test: 各文件 tests

**Interfaces:**
- Produces（Task 2 依赖）:
  ```rust
  pub enum PlayRequest {
      Track(TrackId),
      Playlist(PlaylistId),          // QQ songlist（现有语义不变）
      Album(AlbumId),
      Local(TrackId),
      /// 本地 SQLite 歌单（playlists 表主键；`playlist:local:<id>`）。
      LibraryPlaylist(i64),
  }
  ```

- [ ] **Step 1: 写测试（先行）**

`crates/hmp-core/src/ipc.rs` tests（现有 PlayRequest roundtrip 测试附近）追加：

```rust
    #[test]
    fn library_playlist_roundtrips() {
        let req = PlayRequest::LibraryPlaylist(7);
        let frame = encode_frame(&req).unwrap();
        let back: PlayRequest = decode_frame(&frame).unwrap();
        assert_eq!(back, PlayRequest::LibraryPlaylist(7));
    }
```

`crates/hmp-cli/src/commands.rs` tests（现有 parse_source 测试附近，约 439 行）追加：

```rust
    #[test]
    fn parse_library_playlist_source() {
        assert_eq!(
            parse_source("playlist:local:7"),
            hmp_core::PlayRequest::LibraryPlaylist(7)
        );
        // QQ 歌单语义不变。
        assert!(matches!(
            parse_source("playlist:9001"),
            hmp_core::PlayRequest::Playlist(_)
        ));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-core library_playlist_roundtrips && cargo test -p hmp-cli parse_library_playlist`
Expected: FAIL（编译失败：变体不存在）。

- [ ] **Step 3: 实现**

`crates/hmp-core/src/ipc.rs`：`PlayRequest` 加 `LibraryPlaylist(i64)` 变体（serde derive 自动处理；`PartialEq` derive 已有则 roundtrip 断言可用——确认现有 derive 集含 PartialEq）。

`crates/hmp-cli/src/commands.rs` `parse_source`（**注意顺序**：`playlist:local:` 必须在 `playlist:` 之前匹配）：

```rust
pub fn parse_source(src: &str) -> hmp_core::PlayRequest {
    if let Some(id) = src.strip_prefix("playlist:local:") {
        match id.parse::<i64>() {
            Ok(n) => hmp_core::PlayRequest::LibraryPlaylist(n),
            Err(_) => hmp_core::PlayRequest::Track(hmp_core::TrackId::new(src)),
        }
    } else if let Some(id) = src.strip_prefix("playlist:") {
        …
```

（非法 id 回退单曲——与现有"其他 = 单曲"风格一致。）

- [ ] **Step 4: 跑测试确认通过 + 全量编译**

Run: `cargo test -p hmp-core && cargo test -p hmp-cli && cargo build --workspace`
Expected: 全绿（daemon 的 match 补全在 Task 2——若编译错误出现在 daemon（`PlayRequest` 穷尽 match），Task 1 内先加最小 `_ =>`/显式分支保持编译；**以编译错误清单为准**）。

- [ ] **Step 5: Commit**

```bash
git add crates/hmp-core/src/ipc.rs crates/hmp-cli/src/commands.rs
git commit -m "feat(core,cli): PlayRequest::LibraryPlaylist variant + playlist:local:<id> source syntax"
```

---

### Task 2: 本地歌单解析 + 分发（TDD）

**Files:**
- Modify: `crates/hmp-storage/src/db.rs`（`local_playlist_stubs`）
- Modify: `crates/hmp-daemon/src/local.rs`（LocalSourceResolver 处理 LibraryPlaylist）
- Modify: `crates/hmp-daemon/src/player.rs`（CompositeSourceResolver 分发）
- Test: `crates/hmp-storage/src/db.rs` + `crates/hmp-daemon/src/local.rs` tests

**Interfaces:**
- Consumes: Task 1 变体。
- Produces:
  ```rust
  // db.rs
  /// 本地歌单曲目（播放源）：按 position 排序；JOIN tracks 带完整元数据。
  pub struct LocalPlaylistRow {
      pub source_key: String,
      pub title: String,
      pub artist: Option<String>,
      pub album: Option<String>,
      pub duration_ms: Option<i64>,
  }
  pub fn local_playlist_stubs(&mut self, playlist_id: i64) -> rusqlite::Result<Vec<LocalPlaylistRow>>
      // SELECT t.source_key, t.title, t.artist, t.album, t.duration_ms
      // FROM playlist_tracks pt JOIN tracks t ON t.id = pt.track_id
      // WHERE pt.playlist_id = ?1 ORDER BY pt.position
      // 歌单不存在 → Ok(vec![])（调用方转 PlaylistNotFound）
  ```

- [ ] **Step 1: 写测试（先行）**

`crates/hmp-storage/src/db.rs` tests：

```rust
    #[test]
    fn local_playlist_stubs_lists_ordered_tracks() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let pid = db.create_playlist("本地歌单").unwrap();
        // 混排：QQ 曲目 + 本地曲目。
        db.add_playlist_track(pid, "qq", "mid-1", "QQ 歌").unwrap();
        db.add_playlist_track(pid, "local", "local:/a.mp3", "本地歌").unwrap();
        let rows = db.local_playlist_stubs(pid).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source_key, "mid-1");
        assert_eq!(rows[0].title, "QQ 歌");
        assert_eq!(rows[1].source_key, "local:/a.mp3");
        // 不存在 → 空列表。
        assert!(db.local_playlist_stubs(999).unwrap().is_empty());
    }
```

`crates/hmp-daemon/src/local.rs` tests（现有 resolve_local_album_source 附近风格）：

```rust
    #[tokio::test]
    async fn resolve_library_playlist_source() {
        // 内存库造歌单 → PlayRequest::LibraryPlaylist(pid)
        // → LocalSourceResolver.resolve_source_ids 返回全部 stub。
        let library = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        {
            let mut db = library.lock().unwrap();
            let pid = db.create_playlist("p").unwrap();
            db.add_playlist_track(pid, "qq", "mid-1", "QQ 歌").unwrap();
            db.add_playlist_track(pid, "local", "local:/x.mp3", "本地歌").unwrap();
        }
        let resolver = LocalSourceResolver::new(library);
        let src = hmp_core::PlayRequest::LibraryPlaylist(1);
        let stubs = resolver.resolve_source_ids(&src).await.unwrap();
        assert_eq!(stubs.len(), 2);
        assert_eq!(stubs[0].id.as_ref(), "mid-1");
        assert_eq!(stubs[1].id.as_ref(), "local:/x.mp3");
        assert_eq!(stubs[0].title, "QQ 歌");
    }

    #[tokio::test]
    async fn resolve_library_playlist_missing_is_not_found() {
        let library = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let resolver = LocalSourceResolver::new(library);
        let src = hmp_core::PlayRequest::LibraryPlaylist(999);
        let err = resolver.resolve_source_ids(&src).await.unwrap_err();
        assert!(matches!(err, EngineError::PlaylistNotFound(_)));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-storage local_playlist_stubs && cargo test -p hmp-daemon resolve_library_playlist`
Expected: FAIL（编译失败）。

- [ ] **Step 3: 实现**

1. `crates/hmp-storage/src/db.rs`：`LocalPlaylistRow` + `local_playlist_stubs`（SQL 如上；`playlist_tracks` 表无唯一约束——**ORDER BY pt.position 有同 position 并列风险**：加 `, pt.rowid` 稳定排序——SQLite 每表有 rowid ✓）。
2. `crates/hmp-daemon/src/local.rs`：`resolve_source_ids` 加分支（仿 `album:local:` 分支结构）：

```rust
            // 里程碑 F：`playlist:local:<id>` → 本地歌单曲目（JOIN tracks，混排 QQ/本地）。
            PlayRequest::LibraryPlaylist(id) => {
                let lib = self.library.clone();
                let id = *id;
                Box::pin(async move {
                    let mut db = lib
                        .lock()
                        .map_err(|e| EngineError::Internal(e.to_string()))?;
                    let rows = db
                        .local_playlist_stubs(id)
                        .map_err(|e| EngineError::Internal(e.to_string()))?;
                    if rows.is_empty() {
                        return Err(EngineError::PlaylistNotFound(format!(
                            "本地歌单为空或不存在: {id}"
                        )));
                    }
                    let stubs = rows
                        .into_iter()
                        .map(|r| hmp_core::TrackStub {
                            id: TrackId::new(r.source_key),
                            title: r.title,
                            artists: r.artist.into_iter().collect(),
                            album: r.album,
                            duration_ms: r.duration_ms,
                        })
                        .collect();
                    Ok(stubs)
                })
            }
```

3. `crates/hmp-daemon/src/player.rs` `CompositeSourceResolver::resolve_source_ids`：

```rust
        match src {
            PlayRequest::Local(_) => self.local.resolve_source_ids(src),
            PlayRequest::LibraryPlaylist(_) => self.local.resolve_source_ids(src),
            PlayRequest::Album(id) if id.as_ref().starts_with("local:") => self.local.resolve_source_ids(src),
            _ => self.qq.resolve_source_ids(src),
        }
```

4. `crates/hmp-daemon/src/engine.rs`：`play_source` 无需改（统一入口）；若 `match` 穷尽性编译错误（如 resolve_source_ids_impl 的 QQ 侧 `match src`——player.rs:395 的 `resolve_source_ids_impl` 也要加 `LibraryPlaylist` 分支：返回 Internal 错误"本地歌单走本地解析器"——**以编译错误清单为准补全**）。

- [ ] **Step 4: 跑测试确认通过 + 全量**

Run: `cargo test -p hmp-storage && cargo test -p hmp-daemon && cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

- [ ] **Step 5: Commit**

```bash
git add crates/hmp-storage/src/db.rs crates/hmp-daemon/src/local.rs crates/hmp-daemon/src/player.rs crates/hmp-daemon/src/engine.rs
git commit -m "feat(storage,daemon): library playlist playback - playlist:local:<id> resolves local SQLite playlist (mixed qq/local)"
```

---

### Task 3: 收尾验证

- [ ] **Step 1: 全量验证**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e && cargo test -p hmp-cli --test daemon_cli -- --ignored`
Expected: 全绿。

- [ ] **Step 2: 冒烟（可选）**

```bash
hmp playlist create 测试歌单 && hmp playlist add <id> <qq-mid> && hmp play playlist:local:<id>
```
（若环境无凭证/音频设备可跳过——e2e 已有协议级覆盖。）

- [ ] **Step 3: 核对覆盖**

对照里程碑 F：本地歌单播放源（✓ LibraryPlaylist 变体 + local 解析 + 分发）；混排 QQ/本地（✓ TrackStub.id=source_key，resolve_track 按 provider 分发既有机制）；`playlist:<id>` QQ 语义不变（✓）；重复曲目支持 = 已知限制（add_playlist_track 幂等去重保持，occurrence id 留待后续）。

- [ ] **Step 4: 报告**

报告：changed files、每任务验证、`cargo test --workspace` 总通过数、clippy/fmt 状态、未完成项。不要 git push（父会话统一推送并更新 roadmap）。
