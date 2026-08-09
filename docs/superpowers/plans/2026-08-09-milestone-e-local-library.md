# Local Library Domain Implementation Plan（里程碑 E）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 真正的本地媒体库闭环（审计"媒体库缺口"整节；roadmap 里程碑 E）：本地曲目实体建模（多艺术家/序号/年份/流派/封面）、扫描生命周期（增量 + missing 标记 + 移动指纹）、一等浏览入口（tracks/albums/artists 聚合 + 本地专辑/歌手播放）。

**决策定案（本计划权威）：**
1. **artist 多值 → 拆表** `track_artists(track_id, artist, position)`；`tracks.artist` 保留主歌手（现有代码/历史兼容，TrackRow 不动）。
2. **封面 → 提取成文件**：`LocalMeta.cover: Option<Vec<u8>>`（lofty `pictures()`），CLI scan 侧写入 `<data_dir>/covers/<hash>.jpg`，`tracks.cover_uri` 存 `file://` 路径（storage 无文件系统副作用）。
3. **非 UTF-8 路径**：身份与指纹用 `OsStr` 原始字节哈希（`std::collections::hash_map::DefaultHasher`，SipHash13），显示层维持 lossy 现状。
4. **watcher（notify）拆出 E2**：本期做手动扫描闭环；`scan_roots` 表已建好供 E2 注册根目录。

**Architecture:** schema v3（事务迁移，复用 MIGRATION_V2 模式）→ `local_files` 加 `mtime_ns/fingerprint/last_seen_generation/missing/scan_root_id`，`tracks` 加 `album_artist/track_number/disc_number/year/genre`，新表 `track_artists` + `scan_roots`；`LocalMeta` 扩展（lofty 提取多艺术家/序号/年份/流派/封面）；`scan.rs` 重构为 root 注册 + generation 扫描 + 增量（mtime_ns+size 跳过）+ missing 标记/复位 + 指纹命中更新路径；storage 新增聚合查询；CLI `library` 子命令扩展；`CompositeSourceResolver` 分发 `album:local:` 到 `LocalSourceResolver`。

**Tech Stack:** Rust workspace（hmp-storage / hmp-daemon / hmp-cli）；lofty 0.21（已依赖）；serde_json；std 哈希（无新依赖）。

## Global Constraints

- 保持 `cargo test --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 全绿。
- `TrackRow`/`upsert_track`/`add_local_file` 现有签名**不变**（新增方法；`add_local_file` 行为保持幂等 upsert）。
- schema v3 迁移沿用事务模式：失败整体回滚、`user_version` 不推进。
- 指纹 = `DefaultHasher`（路径字节 + mtime_ns + size）；指纹仅作"移动/改名候选"，命中后仍需校验 mtime+size 一致才复用行。
- 扫描不自动删除 local_files 行（missing=1 标记；外接盘离线数据不丢）。
- 每个 Task 独立 commit（`feat(storage,…)` 前缀 + 中文要点）。
- 测试构造音频文件不需要真实音频内容：空文件 + 音频扩展名即可（`read_meta` 返回 None → 文件名回退路径）。

---

### Task 1: schema v3 迁移（storage）

**Files:**
- Modify: `crates/hmp-storage/src/db.rs`（migrate 函数 + MIGRATION_V3 常量 + TrackRow 结构体加字段）
- Test: `crates/hmp-storage/src/db.rs` tests

**Interfaces:**
- Produces（Task 2/3 依赖）:
  - `tracks` 新列：`album_artist TEXT`、`track_number INTEGER`、`disc_number INTEGER`、`year INTEGER`、`genre TEXT`
  - `local_files` 新列：`mtime_ns INTEGER`、`fingerprint TEXT`、`last_seen_generation INTEGER DEFAULT 0`、`missing INTEGER DEFAULT 0`、`scan_root_id INTEGER`
  - 新表：
    ```sql
    CREATE TABLE track_artists (
      track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
      artist TEXT NOT NULL,
      position INTEGER NOT NULL,
      PRIMARY KEY (track_id, position)
    );
    CREATE TABLE scan_roots (
      id INTEGER PRIMARY KEY,
      path TEXT NOT NULL UNIQUE,        -- canonical 绝对路径
      generation INTEGER NOT NULL DEFAULT 0  -- 下次扫描 generation 号
    );
    ```
  - `TrackRow` 加字段：`album_artist: Option<String>`、`track_number: Option<i64>`、`disc_number: Option<i64>`、`year: Option<i64>`、`genre: Option<String>`（upsert_track SQL 同步；**注意**：TrackRow 有 `Default` 吗？若没有，所有构造点都要加字段——先检查；若构造点过多，用 `#[non_exhaustive]` 不可行——**决策：给 TrackRow 新字段加默认值语义，构造点逐个补 `..Default::default()` 或直接加字段**，以编译错误清单为准）

- [ ] **Step 1: 写迁移测试（先行）**

`crates/hmp-storage/src/db.rs` tests 追加（仿现有 v2 迁移回滚测试，约 1461 行）：

```rust
    #[test]
    fn migration_v3_adds_columns_and_tables() {
        // 新库直接到 v3：列与表齐全。
        let db = LibraryDb::open_in_memory().unwrap();
        let cols: Vec<String> = db
            .conn
            .prepare("PRAGMA table_info(local_files)")
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for want in ["mtime_ns", "fingerprint", "last_seen_generation", "missing", "scan_root_id"] {
            assert!(cols.iter().any(|c| c == want), "local_files 缺列 {want}");
        }
        let tcols: Vec<String> = db
            .conn
            .prepare("PRAGMA table_info(tracks)")
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for want in ["album_artist", "track_number", "disc_number", "year", "genre"] {
            assert!(tcols.iter().any(|c| c == want), "tracks 缺列 {want}");
        }
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('track_artists','scan_roots')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2, "track_artists/scan_roots 表应存在");
    }

    #[test]
    fn migration_v2_to_v3_upgrades_in_place() {
        // 构造 v2 库 → 打开触发迁移 → v3 列存在、旧数据保留。
        let dir = std::env::temp_dir().join(format!("hmp-mig-v3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.sqlite3");
        let _ = std::fs::remove_file(&path);
        {
            let mut conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.execute_batch(MIGRATION_V2).unwrap();
            conn.pragma_update(None, "user_version", 2).unwrap();
            conn.execute(
                "INSERT INTO tracks (source, source_key, title, artist) VALUES ('local', 'local:/a.mp3', 'A', 'Art')",
                [],
            )
            .unwrap();
        }
        let mut db = LibraryDb::open(&path).unwrap();
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "迁移后旧数据保留");
        let v: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 3);
        // 新列可写。
        db.conn
            .execute(
                "UPDATE tracks SET genre='Rock' WHERE id=1",
                [],
            )
            .unwrap();
    }
```

（若 `SCHEMA_V1`/`MIGRATION_V2` 常量在 tests 模块可见性不足，用 `crate::SCHEMA_V1`/`crate::MIGRATION_V2` 或经公开路径；现有 v2 回滚测试 1461 行附近如何构造旧库以它为准。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-storage migration_v3`
Expected: FAIL（新库 user_version 仍为 2，缺列）。

- [ ] **Step 3: 实现迁移**

`crates/hmp-storage/src/db.rs`：

`migrate()` 末尾（`current < 3` 分支，事务模式照抄 v2）：

```rust
    if current < 3 {
        conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            conn.execute_batch(MIGRATION_V3)?;
            conn.pragma_update(None, "user_version", 3)?;
            Ok(())
        })();
        match result {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                conn.execute_batch("ROLLBACK").ok();
                return Err(e);
            }
        }
    }
```

```rust
/// v3：本地媒体库域（里程碑 E）——local_files 文件生命周期列
/// （mtime_ns/指纹/扫描代际/missing/scan_root）+ tracks 完整元数据列 +
/// 多艺术家表 + 扫描根表。
const MIGRATION_V3: &str = r#"
ALTER TABLE local_files ADD COLUMN mtime_ns INTEGER;
ALTER TABLE local_files ADD COLUMN fingerprint TEXT;
ALTER TABLE local_files ADD COLUMN last_seen_generation INTEGER NOT NULL DEFAULT 0;
ALTER TABLE local_files ADD COLUMN missing INTEGER NOT NULL DEFAULT 0;
ALTER TABLE local_files ADD COLUMN scan_root_id INTEGER;
ALTER TABLE tracks ADD COLUMN album_artist TEXT;
ALTER TABLE tracks ADD COLUMN track_number INTEGER;
ALTER TABLE tracks ADD COLUMN disc_number INTEGER;
ALTER TABLE tracks ADD COLUMN year INTEGER;
ALTER TABLE tracks ADD COLUMN genre TEXT;
CREATE TABLE track_artists (
  track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  artist TEXT NOT NULL,
  position INTEGER NOT NULL,
  PRIMARY KEY (track_id, position)
);
CREATE INDEX idx_track_artists_artist ON track_artists(artist);
CREATE TABLE scan_roots (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  generation INTEGER NOT NULL DEFAULT 0
);
"#;
```

**注意**：`CREATE TABLE track_artists` 内 `REFERENCES tracks(id)`——若 SCHEMA_V1 里建表顺序与此冲突（SQLite 允许前向引用 ✓）。`PRIMARY KEY (track_id, position)` 表内复合主键需列约束写法 `track_id INTEGER NOT NULL` ✓（如上）。

`TrackRow` 加 5 字段 + `upsert_track` SQL 同步（INSERT/ON CONFLICT DO UPDATE 两处）——以编译错误清单为准逐个补构造点（`..TrackRow` 不存在的字段；测试中构造点约 5-10 处）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p hmp-storage`
Expected: 全绿（含迁移测试）。

- [ ] **Step 5: 全量 + Commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: 全绿。

```bash
git add crates/hmp-storage/src/db.rs
git commit -m "feat(storage): schema v3 - local library domain columns (file lifecycle, full metadata, track_artists, scan_roots)"
```

---

### Task 2: LocalMeta 扩展 + 扫描生命周期（storage + CLI scan 重构）

**Files:**
- Modify: `crates/hmp-storage/src/local.rs`（LocalMeta 扩展 + lofty 提取）
- Modify: `crates/hmp-storage/src/db.rs`（新增扫描方法）
- Modify: `crates/hmp-cli/src/scan.rs`（root 注册 + generation 扫描 + 增量 + missing + 指纹）
- Test: `crates/hmp-storage/src/db.rs` + `crates/hmp-cli/src/scan.rs` tests

**Interfaces:**
- Consumes: Task 1 的 v3 列。
- Produces（Task 3 依赖）:

```rust
// local.rs
pub struct LocalMeta {
    …现有字段…,
    pub artists: Vec<String>,        // 完整艺术家列表（空 = 无标签）
    pub album_artist: Option<String>,
    pub track_number: Option<u16>,
    pub disc_number: Option<u16>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub cover: Option<Vec<u8>>,      // 内嵌封面原图（lofty pictures 首个 front cover）
}

// db.rs 新增：
pub fn begin_scan(&mut self, root: &Path) -> rusqlite::Result<i64>
    // scan_roots upsert（canonical path）→ generation+1 → 返回 (root_id, generation)
pub fn record_scan_file(
    &mut self,
    root_id: i64,
    generation: i64,
    path: &Path,
    meta: Option<&LocalMeta>,
    fingerprint: &str,
) -> rusqlite::Result<ScanOutcome>   // 见下
pub fn finish_scan(&mut self, root_id: i64, generation: i64) -> rusqlite::Result<u32>
    // UPDATE local_files SET missing=1 WHERE scan_root_id=? AND last_seen_generation<?
    // 返回标记数
pub fn clear_missing(&mut self, track_id: i64) -> rusqlite::Result<()>
pub fn find_by_fingerprint(&mut self, fp: &str) -> rusqlite::Result<Option<(i64, String)>>
    // 返回 (track_id, 原 path)；调用方校验 mtime+size 后复用行
pub enum ScanOutcome { Added, Updated, Skipped, MissingReset }

// CLI scan.rs 重构入口：
pub fn scan_dir(root: &Path, db: &mut LibraryDb) -> Result<ScanReport, Box<dyn std::error::Error>>
pub struct ScanReport { pub added: u32, pub updated: u32, pub skipped: u32, pub missing: u32 }
```

**扫描算法（scan_dir 重构，权威）：**
1. canonicalize root → `begin_scan`（root_id, generation）。
2. 递归收集文件（现有 collect_audio 保留）。
3. 对每个文件：
   - 计算 `fingerprint = hash(canonical 字节 + mtime_ns + size)`；`mtime_ns = modified().duration_since(UNIX_EPOCH).as_nanos()`。
   - 查现有行 `track_id(path)`：
     - 存在且 `mtime_ns == 旧值 && size == 旧值` → `Skipped`（不读标签）。
     - 存在但变了 → `read_meta` → `record_scan_file`（更新元数据/指纹/last_seen_generation/清 missing）→ `Updated`。
     - 不存在 → `find_by_fingerprint(fp)`：命中且该 track 无其他 path 占用 → 复用行更新 path（移动/改名）→ `Updated`（或单独计数 `moved`——并入 updated）；未命中 → 新增 `Added`。
4. 全部完成后 `finish_scan`（标 missing）→ report。

**指纹细节**：`fingerprint` 存 hex 字符串（`format!("{:016x}", hasher.finish())`），collision 概率对移动检测场景可接受；命中后调用方须重读 `mtime_ns+size` 与库中记录比对一致才复用。

**封面写盘（CLI scan 侧）**：meta.cover 非空 → `hmp_storage::data_dir().join("covers")` 建目录，文件名 `format!("{:x}.jpg", hash(cover bytes))`（无扩展名嗅探——统一 `.jpg`；`cover_uri = Some(format!("file://{}", path.display()))`）。同 hash 已存在跳过写。

- [ ] **Step 1: 写测试（先行）**

`crates/hmp-storage/src/db.rs` tests：

```rust
    #[test]
    fn scan_lifecycle_marks_missing_and_resets() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        // 临时目录两个空文件（read_meta None → 文件名回退）。
        let dir = std::env::temp_dir().join(format!("hmp-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.mp3");
        let b = dir.join("b.mp3");
        std::fs::write(&a, b"").unwrap();
        std::fs::write(&b, b"").unwrap();
        // 第一轮：全部新增。
        let root_id = db.begin_scan(&dir).unwrap();
        let gen = db
            .conn
            .query_row("SELECT generation FROM scan_roots WHERE id=?1", [root_id], |r| r.get(0))
            .unwrap();
        db.record_scan_file(root_id, gen, &a, None, "fp-a").unwrap();
        db.record_scan_file(root_id, gen, &b, None, "fp-b").unwrap();
        assert_eq!(db.finish_scan(root_id, gen).unwrap(), 0, "首轮无 missing");
        // 删除 b → 第二轮：b 标 missing。
        std::fs::remove_file(&b).unwrap();
        let root_id2 = db.begin_scan(&dir).unwrap();
        let gen2 = db
            .conn
            .query_row("SELECT generation FROM scan_roots WHERE id=?1", [root_id2], |r| r.get(0))
            .unwrap();
        db.record_scan_file(root_id2, gen2, &a, None, "fp-a").unwrap();
        assert_eq!(db.finish_scan(root_id2, gen2).unwrap(), 1, "b 应标 missing");
        let miss: i64 = db
            .conn
            .query_row("SELECT missing FROM local_files WHERE path LIKE '%b.mp3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(miss, 1);
        // 重扫 b 出现 → missing 复位。
        std::fs::write(&b, b"").unwrap();
        let root_id3 = db.begin_scan(&dir).unwrap();
        let gen3 = db
            .conn
            .query_row("SELECT generation FROM scan_roots WHERE id=?1", [root_id3], |r| r.get(0))
            .unwrap();
        let out = db.record_scan_file(root_id3, gen3, &b, None, "fp-b").unwrap();
        assert!(matches!(out, ScanOutcome::MissingReset));
        db.finish_scan(root_id3, gen3).unwrap();
        let miss: i64 = db
            .conn
            .query_row("SELECT missing FROM local_files WHERE path LIKE '%b.mp3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(miss, 0, "b 已复位");
    }

    #[test]
    fn fingerprint_reuses_row_on_path_change() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("hmp-fp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.mp3");
        let new = dir.join("new.mp3");
        std::fs::write(&old, b"x").unwrap();
        let root_id = db.begin_scan(&dir).unwrap();
        let gen: i64 = db.conn.query_row("SELECT generation FROM scan_roots WHERE id=?1", [root_id], |r| r.get(0)).unwrap();
        db.record_scan_file(root_id, gen, &old, None, "fp-same").unwrap();
        // "移动"：旧路径没了，新路径指纹相同。
        std::fs::rename(&old, &new).unwrap();
        let (tid, _orig) = db.find_by_fingerprint("fp-same").unwrap().unwrap();
        let out = db.record_scan_file(root_id, gen, &new, None, "fp-same").unwrap();
        assert!(matches!(out, ScanOutcome::Updated), "指纹命中复用行");
        let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "不产生孤儿曲目");
        let p: String = db.conn.query_row("SELECT path FROM local_files WHERE track_id=?1", [tid], |r| r.get(0)).unwrap();
        assert_eq!(p, new.to_str().unwrap(), "path 已更新");
    }
```

（fingerprint 语义：测试直接传固定指纹——方法签名接收指纹字符串，哈希计算在 CLI scan 侧；storage 侧测试注入即可。）

`crates/hmp-cli/src/scan.rs` tests（新增）：

```rust
    #[test]
    fn scan_dir_incremental_and_missing() {
        // 临时目录：首扫 added=2 → 加文件 skipped 计数 → 删文件 missing=1。
        // 用真实 scan_dir 入口（读标签失败回退文件名）。
    }
```

（scan.rs 现有无测试模块——新增；注意 scan.rs 在 hmp-cli，测试可访问 data_dir()——测试里直接传临时 dir，不依赖 data_dir。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-storage scan_lifecycle && cargo test -p hmp-cli scan_dir_incremental`
Expected: FAIL（编译失败：方法/字段不存在）。

- [ ] **Step 3: 实现 storage 侧**

`crates/hmp-storage/src/local.rs`：`LocalMeta` 扩展 + `read_meta` 提取（lofty Tag trait：`tag.track()`/`tag.disk()`/`tag.year()`/`tag.genre()` 返回 Option；`tag.artists()` 0.21 是否有？——若无用 `tag.get_string(&TagItemKey::Artist)` 迭代或 `tag.artist()` 单值；**以 lofty 0.21 实际 API 为准**，多艺术家可用 `tag.artist()` 单值 + `tag.get_strings(&TagItemKey::Artist)`（0.21 有 `get_strings`）；`tag.pictures()` → `picture.data()` 首个 front cover（`picture.pic_type()` 为 `PictureType::CoverFront` 或回退首个）。封面取前 2MB 上限防爆内存（`.take(2*1024*1024)` 截断——`Vec<u8>` 无 take，用 `data.chunks(…).next()` 或直接 `if data.len() > 2MB { skip }`）。

`crates/hmp-storage/src/db.rs`：`begin_scan`/`record_scan_file`/`finish_scan`/`clear_missing`/`find_by_fingerprint` + `ScanOutcome`。`record_scan_file` 逻辑：
- `track_id("local", &format!("local:{}", path.display()))` 查现有：
  - 有 → 读旧 mtime_ns/size → 一致 → `Skipped`（更新 last_seen_generation + 清 missing——注意 Skipped 也要清 missing，否则重扫后 missing 复位逻辑错：**Skipped 也 update last_seen_generation/missing=0**，不更新其他）。
  - 有但变了 → upsert 更新（add_local_file 扩展版：带 gen/fingerprint/missing 复位 + 新元数据列 + track_artists 重写）→ `Updated`。
  - 无 → `find_by_fingerprint` 命中 → 复用行（UPDATE path/mtime/size/fingerprint/gen/missing + 元数据）→ `Updated`；未命中 → 新增（tracks 行 + local_files 行 + track_artists）→ `Added`。
- `begin_scan`：`INSERT INTO scan_roots(path) VALUES(?1) ON CONFLICT(path) DO UPDATE SET generation = generation + 1 RETURNING id`（RETURNING 需 SQLite 3.35+——rusqlite bundled 版本？**风险点**：若 bundled SQLite 旧，用两步：先 upsert 再 SELECT id/generation）。返回 (root_id, generation)——签名 `-> Result<(i64, i64)>`（计划上文写 i64 有误——以两步查询实现，返回元组）。
- `finish_scan`：`UPDATE local_files SET missing=1 WHERE scan_root_id=?1 AND last_seen_generation<?2` → 返回改动行数。
- `add_local_file` 内部同步更新 track_artists（artists 列表写入；`tracks.artist` 仍设主值）——**决策：扩展 add_local_file 内部**（签名不变，LocalMeta 扩展后自动生效）——但 add_local_file 不带 gen/fingerprint 参数（旧调用点兼容）→ gen/fingerprint 经 `record_scan_file` 设置；add_local_file 保持原样只更新元数据。**即：record_scan_file 内部复用 add_local_file 的 SQL 逻辑或直接内联**——为减少重复，`record_scan_file` 内联完整 upsert（含新列），`add_local_file` 保持旧行为（新列留空）。可接受（scan 是唯一入口）。

- [ ] **Step 4: 实现 CLI scan.rs 重构**

```rust
/// 扫描报告。
pub struct ScanReport { pub added: u32, pub updated: u32, pub skipped: u32, pub missing: u32 }

pub fn scan_dir(root: &Path, db: &mut LibraryDb) -> Result<ScanReport, Box<dyn std::error::Error>> {
    let dir = root.canonicalize().map_err(|_| format!("不是目录: {}", root.display()))?;
    let (root_id, gen) = db.begin_scan(&dir)?;
    let mut files = Vec::new();
    let mut visited = HashSet::new();
    collect_audio(&dir, &mut visited, &mut files)?;
    let mut report = ScanReport { added: 0, updated: 0, skipped: 0, missing: 0 };
    for path in files {
        let meta = std::fs::metadata(&path)?;
        let mtime_ns = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64).unwrap_or(0);
        let size = meta.len();
        // 指纹：canonical 路径字节 + mtime_ns + size。
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hasher.write(path.as_os_str().as_encoded_bytes());
        hasher.write_u64(mtime_ns);
        hasher.write_u64(size);
        let fp = format!("{:016x}", hasher.finish());
        let local_meta = read_meta(&path);
        match db.record_scan_file(root_id, gen, &path, local_meta.as_ref(), &fp)? {
            ScanOutcome::Added => report.added += 1,
            ScanOutcome::Updated => report.updated += 1,
            ScanOutcome::Skipped => report.skipped += 1,
            ScanOutcome::MissingReset => report.skipped += 1, // 复位不算更新
        }
        // 封面写盘（新增/更新时；cover 提取在 read_meta）。
        if let Some(m) = &local_meta {
            if let Some(cover) = &m.cover {
                let covers = hmp_storage::data_dir().join("covers");
                std::fs::create_dir_all(&covers)?;
                let mut ch = std::collections::hash_map::DefaultHasher::new();
                ch.write(cover);
                let name = format!("{:016x}.jpg", ch.finish());
                let cpath = covers.join(&name);
                if !cpath.exists() {
                    std::fs::write(&cpath, cover)?;
                }
                // 更新 cover_uri（若与现不同）——经 record_scan_file 内更新。
            }
        }
    }
    report.missing = db.finish_scan(root_id, gen)?;
    Ok(report)
}
```

（封面 URI 写入：record_scan_file 无法知道封面文件路径（CLI 侧计算）——**设计修正**：封面写盘在 record_scan_file 调用前完成，然后 `db.set_track_cover(local_path_key, cover_uri)`（新增小方法：`UPDATE tracks SET cover_uri=?2 WHERE source='local' AND source_key=?1`）——仅当 cover_uri 不同才写。加该小方法。）

`run(dir)` 更新：调用 `scan_dir` 打印报告（新增/更新/跳过/缺失计数）。

- [ ] **Step 5: 跑测试 + 全量**

Run: `cargo test -p hmp-storage && cargo test -p hmp-cli && cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

- [ ] **Step 6: Commit**

```bash
git add crates/hmp-storage/src/local.rs crates/hmp-storage/src/db.rs crates/hmp-cli/src/scan.rs
git commit -m "feat(storage,cli): scan lifecycle - incremental scan, missing marking, fingerprint move detection, cover extraction"
```

---

### Task 3: 浏览入口（storage 聚合 + CLI library 子命令 + 本地专辑/歌手播放）

**Files:**
- Modify: `crates/hmp-storage/src/db.rs`（聚合查询）
- Modify: `crates/hmp-cli/src/library.rs`（新命令实现）
- Modify: `crates/hmp-cli/src/main.rs`（LibraryCmd 枚举扩展）
- Modify: `crates/hmp-cli/src/scan.rs`（顶层 `hmp scan` 转发到 library scan——或保留现状 alias）
- Modify: `crates/hmp-daemon/src/local.rs`（LocalSourceResolver 支持 `album:local:` / `artist:local:` 分发）
- Test: 各文件 tests

**Interfaces:**
- Consumes: Task 2 的扫描数据。
- Produces:

```rust
// db.rs
pub struct LibraryTrackRow {
    pub track_id: i64,
    pub source_key: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub missing: bool,
}
pub fn library_tracks(&mut self, search: Option<&str>, artist: Option<&str>, album: Option<&str>, liked_only: bool) -> rusqlite::Result<Vec<LibraryTrackRow>>
pub struct AlbumGroup { pub album: String, pub artist: Option<String>, pub track_count: i64, pub year: Option<i64>, pub cover_uri: Option<String> }
pub fn library_albums(&mut self, search: Option<&str>) -> rusqlite::Result<Vec<AlbumGroup>>
pub struct ArtistGroup { pub artist: String, pub track_count: i64 }
pub fn library_artists(&mut self) -> rusqlite::Result<Vec<ArtistGroup>>
pub fn local_tracks_by_album(&mut self, album: &str) -> rusqlite::Result<Vec<TrackStub-ish>>
pub fn local_tracks_by_artist(&mut self, artist: &str) -> rusqlite::Result<Vec<…>>
pub fn scan_roots(&mut self) -> rusqlite::Result<Vec<String>>
```

查询要点：
- `library_tracks`：`FROM tracks JOIN local_files ON tracks.id = local_files.track_id WHERE source='local'` + 可选过滤（search → title LIKE；artist → artist=? 或 EXISTS track_artists；album → album=?；liked_only → EXISTS relations(track, liked, true)）+ `ORDER BY title`。
- `library_albums`：`GROUP BY album, artist`（album 非空）`ORDER BY album`。
- `library_artists`：`SELECT artist, COUNT(*) FROM track_artists GROUP BY artist ORDER BY artist`（或 tracks.artist 主值 + track_artists 合并——**决策：用 track_artists（完整）**）。
- 本地专辑播放：`local_tracks_by_album` 按 album 名精确匹配 → 返回 Vec<(source_key, title, artist, duration_ms)> → daemon 侧构造 TrackStub。

// CLI main.rs
```rust
    /// 本地媒体库：浏览/搜索/扫描（里程碑 E）。
    #[command(subcommand)]
    Library(LibraryCmd),   // 已有——枚举内扩展：

enum LibraryCmd {
    History { count: Option<u32> },   // 已有
    Sync,                             // 已有
    SyncStatus,                       // 已有
    /// 本地曲目浏览（默认全部本地曲目）。
    Tracks {
        /// 搜索（标题/歌手/专辑子串）。
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        artist: Option<String>,
        #[arg(long)]
        album: Option<String>,
        /// 只看已收藏。
        #[arg(long)]
        liked: bool,
    },
    /// 本地专辑聚合。
    Albums {
        #[arg(long)]
        search: Option<String>,
    },
    /// 本地歌手聚合。
    Artists,
    /// 扫描本地目录入库（注册为扫描根）。
    Scan { dir: String },
}
```

CLI 输出：表格（类似现有 tracks_liked 风格——`{} 首 · {}` 行格式或固定列宽；以现有 library.rs 输出风格为准）。

// daemon local.rs——本地专辑/歌手播放源：
```rust
impl LocalSourceResolver {
    async fn resolve_local_source(&self, src: &PlayRequest) -> Result<Vec<TrackStub>, EngineError> {
        match src {
            PlayRequest::Local(_) => …现有逻辑…,
            PlayRequest::Album(id) if id.as_ref().starts_with("local:") => {
                // 查库：专辑名 == 解码部分 → 曲目列表 → TrackStub
            }
            _ => Err(EngineError::Internal(…)  // 无 artist: 播放源（本期仅专辑）
        }
    }
}
// CompositeSourceResolver::resolve_source_ids 分发：
match src {
    PlayRequest::Local(_) => self.local.resolve_source_ids(src),
    PlayRequest::Album(id) if id.as_ref().starts_with("local:") => self.local.resolve_source_ids(src),
    _ => self.qq.resolve_source_ids(src),
}
```

CLI `parse_source`（commands.rs:223）：`album:local:<名称>` 已被现有 `album:` 前缀解析为 `PlayRequest::Album(AlbumId("local:<名称>"))` ✓ 无需改（本地专辑播放依赖库中已有扫描数据）。

**专辑名匹配**：`local_tracks_by_album(&album)` 精确匹配（大小写不敏感 `COLLATE NOCASE`）；空结果 → `EngineError::PlaylistNotFound("本地专辑为空")`。

- [ ] **Step 1: 写测试（先行）**

storage tests：

```rust
    #[test]
    fn library_browse_aggregations() {
        // 造数据：两个本地曲目（同专辑/不同歌手/多艺术家）→
        // library_tracks 过滤（search/artist/album/liked）、
        // library_albums 聚合（count/year/cover）、library_artists（多值拆行）。
    }

    #[test]
    fn local_tracks_by_album_and_artist() {
        // 按专辑名/歌手名取曲目列表（播放源用）。
    }
```

daemon local.rs tests：

```rust
    #[tokio::test]
    async fn resolve_local_album_source() {
        // 内存库插入本地曲目（同 album）→ PlayRequest::Album("local:某专辑")
        // → LocalSourceResolver.resolve_source_ids 返回全部曲目 stub。
    }
```

CLI：`parse_source("album:local:xx")` 断言（commands.rs 现有 parse_source 测试扩展）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p hmp-storage library_browse && cargo test -p hmp-daemon resolve_local_album`
Expected: FAIL（编译失败）。

- [ ] **Step 3: 实现 storage 聚合 + daemon 分发 + CLI**

按 Interfaces 实现。`library_tracks` 的 liked_only 复用 `relations` 表（`entity_type='track' AND provider='qq'`？——本地曲目的 relations：`relations(entity_type='track', provider='local'?, …)`——检查现有 Favorite 对本地曲目的写法（set_relation("track","qq",…?)——**以现有代码为准**：favorite 的 entity_key 是什么，liked 过滤按同构写）。

- [ ] **Step 4: 跑测试 + 全量**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check && cargo test -p hmp-daemon --test e2e && cargo test -p hmp-cli --test daemon_cli -- --ignored`
Expected: 全绿。

- [ ] **Step 5: Commit**

```bash
git add crates/hmp-storage/src/db.rs crates/hmp-daemon/src/local.rs crates/hmp-cli/src/library.rs crates/hmp-cli/src/main.rs
git commit -m "feat(storage,daemon,cli): local library browse - tracks/albums/artists aggregation + album:local playback"
```

---

### Task 4: 收尾验证

- [ ] **Step 1: 全量验证**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 全绿。

- [ ] **Step 2: 手工冒烟（本地文件库）**

Run（若环境可）：
```bash
mkdir -p /tmp/hmp-e-smoke && cp 任意音频文件 /tmp/hmp-e-smoke/ 2>/dev/null
cargo run -p hmp-cli -- scan /tmp/hmp-e-smoke    # 或 library scan
cargo run -p hmp-cli -- library tracks
cargo run -p hmp-cli -- library albums
cargo run -p hmp-cli -- library artists
```
冒烟非强制（CI 无音频文件也可跳过——空文件+扩展名也走通路径）。

- [ ] **Step 3: 核对覆盖**

对照里程碑 E 目标：实体建模（✓ track_artists/元数据列/封面）、扫描生命周期（✓ 增量/缺失/指纹/复位）、浏览入口（✓ tracks/albums/artists + album:local 播放）、非 UTF-8 字节指纹（✓）、watcher（**拆 E2**——scan_roots 表已备）。身份：本地专辑身份 = `album:local:<名称>`（按名匹配，非 id——注明限制：重名专辑合并；E2 可引入 album_id）。

- [ ] **Step 4: 报告**

报告：changed files、每任务验证、`cargo test --workspace` 总通过数、clippy/fmt 状态、未完成项（E2: watcher、artist:local 播放、专辑 id 化）。不要 git push（父会话统一推送并更新 roadmap）。
