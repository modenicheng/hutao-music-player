# Local Library Watcher Implementation Plan（里程碑 E2）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** daemon 常驻监听 `scan_roots`（里程碑 E 已建表），文件新增/修改/删除自动增量入库——无需手动 `hmp scan`。watch 事件驱动单文件处理（不推进 generation、不标 missing 批量标记）；缺失标记仍由手动全量扫描负责（复制大目录中途不误标）。

**Architecture:**
1. **共享扫描逻辑下沉 hmp-storage**：CLI `scan.rs` 的 `file_fingerprint`/`persist_cover` 移到 `crates/hmp-storage/src/scan.rs`（watcher 与 CLI 复用；`data_dir()`/`read_meta` 已在 hmp-storage）。storage 加 `mark_missing_by_path(path)`（单文件删除标记）。
2. **watcher worker（hmp-daemon 新模块 `watcher.rs`）**：`notify::RecommendedWatcher` 监听所有 scan_roots（递归）；notify handler 线程仅把事件路径收集进共享去重队列（轻量不阻塞）；daemon spawn 后台任务每 1s 消费队列：音频文件 → `record_scan_file`（root_id/当前 gen 不推进）+ 封面；Remove 事件 → `mark_missing_by_path`；Rename 由 notify 拆为 Create(新路径)+Remove(旧路径)——指纹命中自动复用行、旧路径 mark_missing 查无行无操作（天然正确）。
3. **root 刷新**：后台任务每 30s 重读 `scan_roots()`，新增 root 增量注册监听（CLI `hmp library scan` 新目录后 daemon 自动开始监听）；root 目录不存在（外接盘离线）→ warn 跳过，下次刷新重试。
4. **保活**：`Daemon` 结构体持 `LocalWatcher` 字段（drop 即停止）；`serve.rs` 已持 Daemon 到进程结束。

**Tech Stack:** Rust workspace；`notify = "6"`（新依赖，workspace.dependencies + hmp-daemon）；tokio；现有 storage 扫描方法。

## Global Constraints

- 保持 `cargo test --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 全绿。
- watcher **不调用** `begin_scan`/`finish_scan`（不推进 generation、不批量标 missing——复制大目录中途不误标）；单文件处理用 root 当前 generation。
- 非音频事件（目录/其它扩展名）忽略；watcher 启动/运行失败只 warn 不 panic、不阻断 daemon。
- `file_fingerprint`/`persist_cover` 从 hmp-cli 移到 hmp-storage 后，CLI 行为不变（测试平移）。
- 每个 Task 独立 commit（`feat(storage,…)` 前缀 + 中文要点）。

---

### Task 1: 扫描逻辑下沉 hmp-storage（TDD）

**Files:**
- Modify: `crates/hmp-storage/src/scan.rs`（新模块：`file_fingerprint` + `persist_cover`；lib.rs 挂 `pub mod scan`）
- Modify: `crates/hmp-storage/src/db.rs`（`mark_missing_by_path`）
- Modify: `crates/hmp-cli/src/scan.rs`（删本地实现，改调 hmp_storage::scan）
- Test: `crates/hmp-storage/src/scan.rs` tests + `crates/hmp-cli/src/scan.rs` tests（现有测试平移）

**Interfaces:**
- Produces（Task 2 依赖）:
  ```rust
  // crates/hmp-storage/src/scan.rs
  /// 文件指纹：内容 hash（前 1MB）+ size。不含路径与 mtime（移动/改名后复用候选）。
  pub fn file_fingerprint(path: &Path, size: u64) -> std::io::Result<String>
  /// 提取封面到 `<data_dir>/covers/<hash>.jpg`；返回 cover_uri（`file://…`）。
  pub fn persist_cover(cover: &[u8]) -> std::io::Result<String>

  // db.rs
  /// 单文件删除标记（watcher Remove 事件用；不删行，与扫描 missing 语义一致）。
  pub fn mark_missing_by_path(&mut self, path: &Path) -> rusqlite::Result<u32>
      // UPDATE local_files SET missing=1 WHERE path=?1；返回改动行数
  ```

- [ ] **Step 1: 写测试（先行）**

`crates/hmp-storage/src/scan.rs` tests：

```rust
    #[test]
    fn fingerprint_stable_across_moves() {
        let dir = std::env::temp_dir().join(format!("hmp-fp2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.mp3");
        let b = dir.join("b.mp3");
        std::fs::write(&a, b"content-x").unwrap();
        std::fs::write(&b, b"content-x").unwrap();
        let fa = file_fingerprint(&a, std::fs::metadata(&a).unwrap().len()).unwrap();
        let fb = file_fingerprint(&b, std::fs::metadata(&b).unwrap().len()).unwrap();
        assert_eq!(fa, fb, "内容相同则指纹相同（移动检测候选）");
        let diff = dir.join("diff.mp3");
        std::fs::write(&diff, b"other").unwrap();
        let fd = file_fingerprint(&diff, std::fs::metadata(&diff).unwrap().len()).unwrap();
        assert_ne!(fa, fd);
    }

    #[test]
    fn persist_cover_writes_deduplicated_file() {
        let cover = vec![1u8, 2, 3, 4];
        let uri1 = persist_cover(&cover).unwrap();
        let uri2 = persist_cover(&cover).unwrap();
        assert_eq!(uri1, uri2, "同封面去重（同 hash 文件名）");
        assert!(uri1.starts_with("file://"), "{uri1}");
        let p = uri1.strip_prefix("file://").unwrap();
        assert!(std::path::Path::new(p).exists());
    }
```

`crates/hmp-storage/src/db.rs` tests 追加：

```rust
    #[test]
    fn mark_missing_by_path_flags_row() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("hmp-mm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.mp3");
        std::fs::write(&f, b"x").unwrap();
        let (root_id, gen) = db.begin_scan(&dir).unwrap();
        db.record_scan_file(root_id, gen, &f, None, "fp").unwrap();
        let n = db.mark_missing_by_path(&f).unwrap();
        assert_eq!(n, 1);
        let miss: i64 = db
            .conn
            .query_row("SELECT missing FROM local_files WHERE path=?1", [f.to_str().unwrap()], |r| r.get(0))
            .unwrap();
        assert_eq!(miss, 1);
        // 已删除文件的路径 → 0 行。
        assert_eq!(db.mark_missing_by_path(&dir.join("gone.mp3")).unwrap(), 0);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-storage fingerprint_stable && cargo test -p hmp-storage mark_missing`
Expected: FAIL（模块/方法不存在）。

- [ ] **Step 3: 实现**

1. `crates/hmp-storage/src/scan.rs`：从 `crates/hmp-cli/src/scan.rs` 平移 `file_fingerprint`/`persist_cover`（含 use；`persist_cover` 返回类型从 `Result<Option<String>, Box<dyn Error>>` 简化为 `std::io::Result<String>`——调用方适配；`data_dir()` 已在 crate 内）。`lib.rs` 加 `pub mod scan;`。
2. `crates/hmp-storage/src/db.rs`：`mark_missing_by_path`。
3. `crates/hmp-cli/src/scan.rs`：删本地 `file_fingerprint`/`persist_cover` 实现，`use hmp_storage::scan::{file_fingerprint, persist_cover};`；`persist_cover` 调用点适配（`m.cover.as_deref().map(persist_cover).transpose()?.flatten()` → 现在返回 String 非 Option：`map(|c| persist_cover(c).map(Some)).transpose()?.flatten()` 或调整）。现有 4 个测试平移（fingerprint_matches_path_change 依赖 file_fingerprint 语义不变 ✓）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p hmp-storage && cargo test -p hmp-cli`
Expected: 全绿。

- [ ] **Step 5: Commit**

```bash
git add crates/hmp-storage/src/scan.rs crates/hmp-storage/src/db.rs crates/hmp-storage/src/lib.rs crates/hmp-cli/src/scan.rs
git commit -m "feat(storage): shared scan helpers (fingerprint/cover) + mark_missing_by_path for watcher reuse"
```

---

### Task 2: watcher worker + daemon 集成（TDD）

**Files:**
- Modify: `Cargo.toml`（workspace.dependencies 加 `notify = "6"`）
- Modify: `crates/hmp-daemon/Cargo.toml`（`notify = { workspace = true }`）
- Add: `crates/hmp-daemon/src/watcher.rs`
- Modify: `crates/hmp-daemon/src/daemon.rs`（spawn + 保活字段）、`crates/hmp-daemon/src/lib.rs`（`mod watcher;`）
- Test: `crates/hmp-daemon/src/watcher.rs` tests（真实文件事件集成）

**Interfaces:**
- Consumes: Task 1 的 `file_fingerprint`/`persist_cover`/`mark_missing_by_path`；storage `scan_roots()`/`begin_scan` 的 root 查询（`record_scan_file` 需 root_id/gen——用 `db.scan_roots()` 的已有查询或新增 `scan_root_of(path) -> Option<(i64, i64)>`？**决策：新增轻量方法** `scan_root_id_and_generation(&mut self, canonical_dir: &Path) -> Option<(i64, i64)>`（按 path 精确匹配 scan_roots）。
- Produces:
  ```rust
  // watcher.rs
  /// 本地目录监听器：scan_roots 变化自动入库（里程碑 E2）。
  /// 事件驱动单文件处理（不推进 generation、不批量标 missing）。
  pub struct LocalWatcher { … }   // drop = 停止监听

  impl LocalWatcher {
      /// 启动监听（读 scan_roots；无 root 或无库 → None）。后台批处理任务随结构保活。
      pub fn spawn(library: Arc<Mutex<LibraryDb>>) -> Option<Self>
      /// 处理单个文件路径（新增/修改）；返回是否入库（非音频 → false）。
      fn handle_upsert(&self, path: &Path) -> Result<bool, …>
      /// 处理删除事件。
      fn handle_remove(&self, path: &Path)
  }
  ```

- [ ] **Step 1: 写集成测试（先行）**

`crates/hmp-daemon/src/watcher.rs` tests：

```rust
    use super::*;
    use hmp_storage::LibraryDb;

    fn temp_env() -> (tempfile::TempDir, tempfile::TempDir) {
        // (库目录, 音乐目录)：库用文件库（scan_roots 持久），音乐目录空。
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap())
    }

    #[tokio::test]
    async fn watcher_ingests_new_and_deleted_files() {
        let (lib_dir, music) = temp_env();
        let db_path = lib_dir.path().join("library.sqlite3");
        let mut db = LibraryDb::open(&db_path).unwrap();
        // 注册扫描根（模拟 CLI scan 注册）。
        db.begin_scan(music.path()).unwrap();
        drop(db);
        let library = Arc::new(Mutex::new(LibraryDb::open(&db_path).unwrap()));
        let watcher = LocalWatcher::spawn(library.clone()).expect("应启动监听");
        // 写入音频文件 → 事件 → 自动入库（轮询等待，超时 5s）。
        let f = music.path().join("new-song.mp3");
        std::fs::write(&f, b"abc").unwrap();
        let key = format!("local:{}", f.canonicalize().unwrap().display());
        let mut found = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let lib = library.lock().unwrap();
            if lib.track_id("local", &key).unwrap().is_some() {
                found = true;
                break;
            }
        }
        assert!(found, "新增文件应自动入库: {key}");
        // 删除 → missing 标记（轮询等待）。
        std::fs::remove_file(&f).unwrap();
        let mut missing = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let lib = library.lock().unwrap();
            let miss: i64 = lib
                .conn
                .query_row(
                    "SELECT missing FROM local_files WHERE path=?1",
                    [f.to_str().unwrap()],
                    |r| r.get(0),
                )
                .unwrap_or(-1);
            if miss == 1 {
                missing = true;
                break;
            }
        }
        assert!(missing, "删除文件应标 missing");
        drop(watcher);
    }

    #[tokio::test]
    async fn watcher_ignores_non_audio_and_no_roots() {
        // 无 scan_roots → spawn 返回 None。
        let (lib_dir, _) = temp_env();
        let db_path = lib_dir.path().join("library.sqlite3");
        let library = Arc::new(Mutex::new(LibraryDb::open(&db_path).unwrap()));
        assert!(LocalWatcher::spawn(library.clone()).is_none(), "无 root 不启动");
        // 非音频文件事件不入库。
        let music = tempfile::tempdir().unwrap();
        let mut db = library.lock().unwrap();
        db.begin_scan(music.path()).unwrap();
        drop(db);
        let watcher = LocalWatcher::spawn(library.clone()).expect("有 root 启动");
        let f = music.path().join("note.txt");
        std::fs::write(&f, b"hello").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let lib = library.lock().unwrap();
        let n: i64 = lib
            .conn
            .query_row("SELECT COUNT(*) FROM tracks WHERE source='local'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "非音频不应入库");
    }
```

注意：`notify` 事件到达是异步的（inotify 延迟毫秒级）；轮询等待模式已处理。`tempfile` 是 hmp-daemon 现有 dev-dependency ✓。测试可能受 CI 环境 inotify 限制——若 CI 无 inotify（极少数容器），测试可标 `#[ignore]` 加注释；**默认不 ignore**（Linux 桌面环境正常）。若 watch 失败（`notify` 返回 Err——如目录在容器中不可监听），spawn 里 warn 并返回 None——测试 1 会失败？不会——测试 1 spawn 必须成功（expect）……若环境不支持，expect panic。折衷：测试 1 的 spawn 用 `if let Some(w) = … else { eprintln!("跳过：环境不支持"); return; }`——**决策：spawn 失败（无 root / watch 错误）时测试优雅跳过**。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-daemon watcher_ingests`
Expected: FAIL（编译失败：LocalWatcher 不存在）。

- [ ] **Step 3: 实现 watcher.rs**

```rust
//! 本地目录监听（里程碑 E2）：scan_roots 变化自动入库。
//!
//! 事件驱动单文件处理：不推进 generation、不批量标 missing（缺失标记
//! 由手动全量扫描负责——复制大目录中途不会被误标）。删除事件 → 单文件
//! missing 标记；移动 = notify 的 Create(新路径)+Remove(旧路径) → 指纹
//! 命中自动复用行、旧路径 mark_missing 查无行无操作。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hmp_storage::{LibraryDb, ScanOutcome, read_meta};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// 监听器句柄（drop = 停止监听与批处理任务）。
pub struct LocalWatcher {
    _watcher: RecommendedWatcher,
    _task: tokio::task::JoinHandle<()>,
}

/// 事件批处理队列（notify handler 线程只收集；后台任务每 1s 消费）。
#[derive(Default)]
struct WatchQueue {
    pending: Mutex<HashSet<PathBuf>>,
}

impl WatchQueue {
    fn push(&self, p: PathBuf) {
        self.pending.lock().unwrap().insert(p);
    }
    fn drain(&self) -> Vec<PathBuf> {
        std::mem::take(&mut *self.pending.lock().unwrap()).into_iter().collect()
    }
}

impl LocalWatcher {
    /// 启动监听。无 scan_roots / 全部 root 监听失败 → None（warn 不阻断 daemon）。
    pub fn spawn(library: Arc<Mutex<LibraryDb>>) -> Option<Self> {
        let roots: Vec<String> = {
            let lib = library.lock().unwrap();
            match lib.scan_roots() {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(%e, "读取 scan_roots 失败");
                    return None;
                }
            }
        };
        if roots.is_empty() {
            return None;
        }
        let queue = Arc::new(WatchQueue::default());
        let (tx, mut rx) = mpsc::unbounded_channel::<()>(); // 唤醒批处理任务
        // notify handler：仅收集路径（轻量，不阻塞 notify 线程）。
        let handler_queue = Arc::clone(&queue);
        let handler_tx = tx.clone();
        let mut watcher = match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(ev) = res {
                match ev.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        for p in ev.paths {
                            if hmp_storage::is_audio_ext(&p) {
                                handler_queue.push(p);
                            }
                        }
                    }
                    EventKind::Remove(_) => {
                        for p in ev.paths {
                            handler_queue.push(p); // 删除也进队（音频过滤在消费侧）
                        }
                    }
                    _ => {}
                }
                let _ = handler_tx.send(());
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(%e, "notify watcher 创建失败");
                return None;
            }
        }
        let mut watched: HashSet<PathBuf> = HashSet::new();
        for root in &roots {
            let p = PathBuf::from(root);
            match watcher.watch(&p, RecursiveMode::Recursive) {
                Ok(()) => { watched.insert(p); }
                Err(e) => tracing::warn!(%e, root = %p.display(), "监听扫描根失败（外接盘离线？）"),
            }
        }
        if watched.is_empty() {
            return None;
        }
        // 后台批处理任务：每 1s 或收到唤醒即消费队列。
        let task_library = Arc::clone(&library);
        let task_queue = Arc::clone(&queue);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = rx.recv() => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
                let paths = task_queue.drain();
                if paths.is_empty() { continue; }
                let mut lib = match task_library.lock() {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                for p in paths {
                    Self::handle_event(&mut lib, &p);
                }
                drop(lib);
                Self::refresh_roots(&task_library, &mut watcher, &mut watched).await;
            }
        });
        Some(Self { _watcher: watcher, _task: task })
    }

    /// 消费一个事件路径（音频 → upsert/指纹复用；删除 → missing 标记）。
    fn handle_event(lib: &mut LibraryDb, p: &PathBuf) {
        if p.exists() {
            if !hmp_storage::is_audio_ext(p) { return; }
            // 找到所属 root（前缀匹配）拿 root_id/当前 gen（不推进）。
            let Ok(Some((root_id, gen))) = lib.scan_root_for(p) else { return };
            let md = match std::fs::metadata(p) { Ok(m) => m, Err(_) => return };
            let size = md.len();
            let fp = hmp_storage::scan::file_fingerprint(p, size).unwrap_or_default();
            let local_meta = read_meta(p);
            let cover_uri = match &local_meta {
                Some(m) => m.cover.as_deref().and_then(|c| hmp_storage::scan::persist_cover(c).ok()),
                None => None,
            };
            match lib.record_scan_file(root_id, gen, p, local_meta.as_ref(), &fp) {
                Ok(_) => {
                    if let Some(uri) = &cover_uri {
                        let _ = lib.set_track_cover(&format!("local:{}", p.display()), uri);
                    }
                }
                Err(e) => tracing::warn!(%e, path = %p.display(), "watcher 入库失败"),
            }
        } else if hmp_storage::is_audio_ext(p) {
            match lib.mark_missing_by_path(p) {
                Ok(n) => {
                    if n > 0 { tracing::debug!(path = %p.display(), "文件缺失标记"); }
                }
                Err(e) => tracing::warn!(%e, path = %p.display(), "watcher 删除标记失败"),
            }
        }
    }

    /// 周期刷新 scan_roots：新增 root 增量注册监听。
    async fn refresh_roots(
        library: &Arc<Mutex<LibraryDb>>,
        watcher: &mut RecommendedWatcher,
        watched: &mut HashSet<PathBuf>,
    ) {
        let roots: Vec<String> = library.lock().ok().and_then(|l| l.scan_roots().ok()).unwrap_or_default();
        for root in roots {
            let p = PathBuf::from(root);
            if watched.contains(&p) { continue; }
            match watcher.watch(&p, RecursiveMode::Recursive) {
                Ok(()) => { watched.insert(p); }
                Err(e) => tracing::warn!(%e, root = %p.display(), "监听新扫描根失败"),
            }
        }
    }
}
```

（`scan_root_for` 是 storage 新方法：按 path 前缀匹配 scan_roots 返回 (id, generation)——**若嫌前缀匹配复杂，简化为**：`scan_root_for(path)` 遍历 scan_roots，canonicalize(path) 以 root 为前缀。放 db.rs。）

- [ ] **Step 4: storage 补 `scan_root_for`**

`crates/hmp-storage/src/db.rs`：

```rust
    /// 路径所属扫描根（canonical 前缀匹配）→ (root_id, 当前 generation)。
    /// 供 watcher 事件处理：单文件入库用 root 当前代际，不推进 generation。
    pub fn scan_root_for(&mut self, path: &Path) -> rusqlite::Result<Option<(i64, i64)>> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let roots: Vec<(i64, String, i64)> = {
            let mut stmt = self.conn.prepare("SELECT id, path, generation FROM scan_roots")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        Ok(roots
            .into_iter()
            .find(|(_, root, _)| canonical.starts_with(std::path::Path::new(root)))
            .map(|(id, _, gen)| (id, gen)))
    }
```

- [ ] **Step 5: daemon.rs 集成**

```rust
    pub struct Daemon {
        pub handle: EngineHandle,
        /// 本地目录监听（保活：drop 即停止；serve.rs 持 Daemon 到进程结束）。
        _watcher: Option<crate::watcher::LocalWatcher>,
    }
```
`Daemon::start` 末尾（library 打开成功后——**注意**：library 回退内存库时 watcher 无意义（内存库 scan_roots 空 → spawn None，自然处理））：

```rust
        // 本地目录监听（E2）：scan_roots 变化自动入库；无 root 时静默不启动。
        let watcher = crate::watcher::LocalWatcher::spawn(library.clone());
        Ok(Self { handle, _watcher: watcher })
```

`lib.rs` 加 `mod watcher;`。若 lib.rs 无 mod 列表（模块在 src 自动发现）——检查现有模块声明方式（daemon.rs 是 pub mod？）。**以现有为准**：lib.rs 里 `pub mod daemon;` 等显式声明 → 加 `mod watcher;`（或 pub mod 视其他模块）。

- [ ] **Step 6: 跑测试 + 全量**

Run: `cargo test -p hmp-daemon watcher_ && cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e`
Expected: 全绿（watcher 集成测试含真实 inotify 事件，轮询等待）。

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/hmp-daemon/Cargo.toml crates/hmp-daemon/src/watcher.rs crates/hmp-daemon/src/daemon.rs crates/hmp-daemon/src/lib.rs crates/hmp-storage/src/db.rs
git commit -m "feat(daemon): local library watcher - notify watch on scan_roots, auto-ingest add/modify/remove"
```

---

### Task 3: 收尾验证

- [ ] **Step 1: 全量验证**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e && cargo test -p hmp-cli --test daemon_cli -- --ignored`
Expected: 全绿。

- [ ] **Step 2: 手工冒烟（可选，环境允许）**

```bash
cargo run -p hmp-cli -- scan /tmp/hmp-e-smoke   # 注册 root
# 另开终端：cargo run -p hmp-cli -- serve（daemon 前台）
# 向 /tmp/hmp-e-smoke 复制音频文件 → library tracks 出现新曲目
```
（daemon 需已运行；冒烟非强制。）

- [ ] **Step 3: 核对覆盖**

对照 E2 目标：watch 监听（✓ 递归 + 周期刷新新增 root）、自动入库（✓ Create/Modify 单文件 upsert + 封面）、删除标 missing（✓ Remove 事件）、复制大目录不误标（✓ 不调 finish_scan）、非音频忽略（✓）、外接盘离线（✓ watch 失败 warn 跳过 + 刷新重试）、保活（✓ Daemon 字段）。

- [ ] **Step 4: 报告**

报告：changed files、每任务验证、`cargo test --workspace` 总通过数、clippy/fmt 状态、未完成项。不要 git push（父会话统一推送并更新 roadmap）。
