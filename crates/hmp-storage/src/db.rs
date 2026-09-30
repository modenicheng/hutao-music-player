//! SQLite 媒体库（docs/PROJECT.md §5.2 存储层扩展）。
//!
//! 原则（媒体库重构计划 B1）：
//! - 只存**稳定身份与元数据**，绝不写临时播放 URL（QQ 取流 URI 会失效，
//!   播放时仍经 resolver 重新取流）；
//! - 播放历史用**会话粒度**：`record_play_start` INSERT 一条 play_events，
//!   结束/换曲时 `record_play_end` UPDATE（ended_at/listened_ms/end_reason）
//!   并累加 tracks.play_count/last_played_at——禁止按 position 轮询写库；
//! - 迁移用 `PRAGMA user_version` 逐级升级。

use std::path::Path;

use rusqlite::{Connection, params};

/// 曲目行（窄投影：调用方从 `hmp_core::Track` 映射，存储层不依赖媒体模型）。
#[derive(Clone, Debug)]
pub struct TrackRow {
    /// 来源：`qq` | `local`。
    pub source: &'static str,
    /// 来源身份：QQ mid / 本地文件标识。
    pub source_key: String,
    pub title: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    /// 毫秒。
    pub duration_ms: Option<i64>,
    pub cover_uri: Option<String>,
    /// QQ numeric song id（comment biz_id 映射；仅 qq 源有）。
    pub qq_song_id: Option<i64>,
    /// 完整元数据列（v3，本地媒体库域）：专辑艺术家/曲目号/碟号/年份/流派。
    pub album_artist: Option<String>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
    pub year: Option<i64>,
    pub genre: Option<String>,
}

impl Default for TrackRow {
    fn default() -> Self {
        Self {
            source: "local",
            source_key: String::new(),
            title: String::new(),
            album: None,
            artist: None,
            duration_ms: None,
            cover_uri: None,
            qq_song_id: None,
            album_artist: None,
            track_number: None,
            disc_number: None,
            year: None,
            genre: None,
        }
    }
}

/// 批量元数据查询结果（投影层，`track_meta_batch`）。
#[derive(Clone, Debug)]
pub struct TrackMeta {
    pub source: String,
    pub source_key: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// 时长（毫秒；AUDIT §8.11：QQ stub 缓存落库的时长有了批量读出口）。
    pub duration_ms: Option<i64>,
    /// 封面 URI（本地 `file://`；QQ 播放路径写入的远程 URL）。
    pub cover_uri: Option<String>,
}

/// 关系行（收藏/订阅 = durable outbox 一体；`relations` 表）。
#[derive(Clone, Debug)]
pub struct RelationRow {
    pub entity_type: String,
    pub provider: String,
    pub entity_key: String,
    pub relation: String,
    pub desired_state: bool,
    pub last_remote_state: Option<bool>,
    pub sync_state: String,
    pub retry_count: i64,
    pub last_sync_error: Option<String>,
    pub updated_at: i64,
}

/// owned 歌单曲目操作 outbox 行。
#[derive(Clone, Debug)]
pub struct PlaylistOpRow {
    pub id: i64,
    pub playlist_id: i64,
    pub op: String,
    /// QQ mid（song_id 未知时补全用）。
    pub song_key: Option<String>,
    pub song_id: Option<i64>,
    pub sync_state: String,
    pub retry_count: i64,
    pub last_error: Option<String>,
    pub updated_at: Option<i64>,
}

/// 播放会话结束记录。
#[derive(Clone, Debug)]
pub struct PlayEnd {
    pub track_id: i64,
    /// 结束时间戳（秒）。
    pub ended_at: i64,
    /// 实际收听毫秒。
    pub listened_ms: i64,
    /// 结束原因：`ended|next|previous|stop|manual|quit`。
    pub reason: &'static str,
}

/// 最近播放条目（历史查询结果）。
#[derive(Clone, Debug)]
pub struct RecentPlay {
    pub track_id: i64,
    pub title: String,
    pub artist: Option<String>,
    /// 来源（`qq` | `local`）与来源身份（QQ mid / `local:<路径>`）。
    /// 播放键：GUI 历史页整表播放靠它回查（仅 sqlite row id 时 QQ 行无法重建）。
    pub source: String,
    pub source_key: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub listened_ms: i64,
    pub reason: String,
}

/// 收藏条目（列表查询结果）。
#[derive(Clone, Debug)]
pub struct FavoriteRow {
    pub track_id: i64,
    pub source: String,
    pub source_key: String,
    pub title: String,
    pub created_at: Option<i64>,
}

/// 本地歌单条目（列表查询结果）。
#[derive(Clone, Debug)]
pub struct PlaylistRow {
    pub id: i64,
    pub name: String,
    pub created_at: Option<i64>,
    pub track_count: i64,
    /// 来源：`local` | `qq`。
    pub provider: String,
    /// 远端身份（owned=QQ dirid；subscribed=disstid；local 为 NULL）。
    pub remote_id: Option<String>,
    /// 归属：`local` | `owned` | `subscribed`。
    pub relation: String,
    /// 同步状态：`synced` | `pending` | `error`（local 恒 synced）。
    pub sync_state: String,
    pub retry_count: i64,
    pub last_sync_error: Option<String>,
    /// 最近一次状态变更时间（退避节流用）。
    pub updated_at: Option<i64>,
    /// 封面本地产物 URI（`file://…`；reconcile 取得后回写，NULL = 尚未获取）。
    pub cover_uri: Option<String>,
}

/// 本地歌单内曲目。
#[derive(Clone, Debug)]
pub struct PlaylistTrackRow {
    pub position: i64,
    pub track_id: i64,
    pub title: String,
    pub source_key: String,
}

/// 本地歌单曲目（播放源投影，里程碑 F）。
#[derive(Clone, Debug)]
pub struct LocalPlaylistRow {
    pub source_key: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
}

/// 媒体库（进程内单一连接；跨任务共享用 `Arc<Mutex<LibraryDb>>`，WAL 允许
/// 多进程并发读写——daemon 写入、CLI 读取）。
pub struct LibraryDb {
    conn: Connection,
}

/// 扫描结果分类（里程碑 E）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanOutcome {
    /// 新曲目入库。
    Added,
    /// 已有曲目元数据/路径更新（含指纹命中复用）。
    Updated,
    /// mtime+size 未变，跳过（增量）。
    Skipped,
    /// missing 标记复位（文件重新出现）。
    MissingReset,
}

/// 本地曲目浏览行（里程碑 E 浏览入口）。
#[derive(Clone, Debug)]
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

/// 本地专辑聚合行。
#[derive(Clone, Debug)]
pub struct AlbumGroup {
    pub album: String,
    pub artist: Option<String>,
    pub track_count: i64,
    pub year: Option<i64>,
    pub cover_uri: Option<String>,
}

/// 本地歌手聚合行（track_artists 多值拆行）。
#[derive(Clone, Debug)]
pub struct ArtistGroup {
    pub artist: String,
    pub track_count: i64,
}

const SCHEMA_V1: &str = r#"
CREATE TABLE tracks (
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL,
  source_key TEXT NOT NULL,
  title TEXT NOT NULL,
  album TEXT,
  artist TEXT,
  duration_ms INTEGER,
  cover_uri TEXT,
  play_count INTEGER NOT NULL DEFAULT 0,
  last_played_at INTEGER,
  UNIQUE(source, source_key)
);
CREATE TABLE local_files (
  track_id INTEGER PRIMARY KEY REFERENCES tracks(id),
  path TEXT NOT NULL UNIQUE,
  file_size INTEGER,
  mtime INTEGER,
  format TEXT,
  bitrate INTEGER,
  sample_rate INTEGER
);
CREATE TABLE favorites (
  track_id INTEGER PRIMARY KEY REFERENCES tracks(id),
  created_at INTEGER
);
CREATE TABLE playlists (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  created_at INTEGER,
  updated_at INTEGER
);
CREATE TABLE playlist_tracks (
  playlist_id INTEGER REFERENCES playlists(id),
  track_id INTEGER REFERENCES tracks(id),
  position INTEGER,
  added_at INTEGER
);
CREATE TABLE play_events (
  id INTEGER PRIMARY KEY,
  track_id INTEGER REFERENCES tracks(id),
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  listened_ms INTEGER NOT NULL DEFAULT 0,
  end_reason TEXT
);
CREATE INDEX idx_play_events_started ON play_events(started_at DESC);
CREATE INDEX idx_play_events_open ON play_events(end_reason) WHERE ended_at IS NULL;
"#;

impl LibraryDb {
    /// 打开（或创建）库：建目录、启用 WAL、迁移到最新版本。
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // WAL 只允许并发读、写仍互斥；默认 busy_timeout=0 会让 CLI 写操作在
        // daemon 持写锁瞬间直接报 "database is locked"。给跨进程竞争留等待窗口。
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// 内存库（测试）。
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// 当前迁移版本（测试断言）。
    pub fn version(&self) -> rusqlite::Result<i64> {
        self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))
    }

    /// v5 迁移步骤（幂等）：合并本地意图幽灵行（见 [`merge_ghost_local_tracks`]）。
    /// 独立暴露供迁移测试直接驱动。
    pub fn merge_ghost_local_tracks(&mut self) -> rusqlite::Result<()> {
        merge_ghost_local_tracks(&self.conn)
    }

    /// 幂等写入/更新曲目元数据；返回 track id。
    pub fn upsert_track(&mut self, t: &TrackRow) -> rusqlite::Result<i64> {
        let source_key = canonical_local_key(t.source, &t.source_key);
        self.conn.execute(
            r#"INSERT INTO tracks (source, source_key, title, album, artist, duration_ms, cover_uri, album_artist, track_number, disc_number, year, genre)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
               ON CONFLICT(source, source_key) DO UPDATE SET
                 title = CASE
                   -- 意图方不知真实标题时以（原始）键代之，不得踩掉已有元数据
                   WHEN excluded.title = ?13 THEN tracks.title
                   ELSE excluded.title END,
                 album = COALESCE(excluded.album, tracks.album),
                 artist = COALESCE(excluded.artist, tracks.artist),
                 duration_ms = COALESCE(excluded.duration_ms, tracks.duration_ms),
                 cover_uri = COALESCE(excluded.cover_uri, tracks.cover_uri),
                 album_artist = COALESCE(excluded.album_artist, tracks.album_artist),
                 track_number = COALESCE(excluded.track_number, tracks.track_number),
                 disc_number = COALESCE(excluded.disc_number, tracks.disc_number),
                 year = COALESCE(excluded.year, tracks.year),
                 genre = COALESCE(excluded.genre, tracks.genre)"#,
            params![
                t.source,
                source_key,
                t.title,
                t.album,
                t.artist,
                t.duration_ms,
                t.cover_uri,
                t.album_artist,
                t.track_number,
                t.disc_number,
                t.year,
                t.genre,
                t.source_key
            ],
        )?;
        self.conn.query_row(
            "SELECT id FROM tracks WHERE source = ?1 AND source_key = ?2",
            params![t.source, source_key],
            |r| r.get(0),
        )
    }

    /// 记录播放会话开始（INSERT play_events），返回事件 id（供结束按 id 精确
    /// 闭合——同一曲目连续播放产生独立会话，不再按 track_id 猜测）。
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

    /// 启动恢复：闭合遗留的未结束会话（daemon 异常退出/被杀后
    /// `ended_at IS NULL` 的行），`end_reason='interrupted'`、时长 0。
    /// 返回闭合行数（幂等：再次调用返回 0）。
    pub fn close_stale_sessions(&mut self) -> rusqlite::Result<u32> {
        let n = self.conn.execute(
            "UPDATE play_events SET ended_at = started_at, end_reason = 'interrupted' \
             WHERE ended_at IS NULL",
            [],
        )?;
        Ok(n as u32)
    }

    /// 最近播放（默认按开始时间倒序）。带 source/source_key 播放键投影
    /// （GUI 历史页整表播放的 id 来源，与收藏页 `list_favorites` 同口径）。
    pub fn recent_plays(&mut self, limit: u32) -> rusqlite::Result<Vec<RecentPlay>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT p.track_id, t.title, t.artist, t.source, t.source_key,
                      p.started_at, p.ended_at, p.listened_ms, COALESCE(p.end_reason, '')
               FROM play_events p JOIN tracks t ON t.id = p.track_id
               ORDER BY p.started_at DESC, p.id DESC LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(RecentPlay {
                track_id: r.get(0)?,
                title: r.get(1)?,
                artist: r.get(2)?,
                source: r.get(3)?,
                source_key: r.get(4)?,
                started_at: r.get(5)?,
                ended_at: r.get(6)?,
                listened_ms: r.get(7)?,
                reason: r.get(8)?,
            })
        })?;
        rows.collect()
    }

    /// 最近播放（LRU 视图）：一曲一行，按最近一次播放倒序——播放即移到最前，
    /// 同曲重复播放不产生重复行（行字段取该曲最近一次会话）。
    /// GUI 历史页用这份；会话流水（含同曲多条）走 [`recent_plays`]（CLI history）。
    pub fn recent_tracks(&mut self, limit: u32) -> rusqlite::Result<Vec<RecentPlay>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT p.track_id, t.title, t.artist, t.source, t.source_key,
                      p.started_at, p.ended_at, p.listened_ms, COALESCE(p.end_reason, '')
               FROM (
                   SELECT *, ROW_NUMBER() OVER (
                       PARTITION BY track_id ORDER BY started_at DESC, id DESC
                   ) AS rn
                   FROM play_events
               ) p
               JOIN tracks t ON t.id = p.track_id
               WHERE p.rn = 1
               ORDER BY p.started_at DESC, p.id DESC
               LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(RecentPlay {
                track_id: r.get(0)?,
                title: r.get(1)?,
                artist: r.get(2)?,
                source: r.get(3)?,
                source_key: r.get(4)?,
                started_at: r.get(5)?,
                ended_at: r.get(6)?,
                listened_ms: r.get(7)?,
                reason: r.get(8)?,
            })
        })?;
        rows.collect()
    }
    pub fn track_id(&mut self, source: &str, source_key: &str) -> rusqlite::Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM tracks WHERE source = ?1 AND source_key = ?2",
                params![source, source_key],
                |r| r.get(0),
            )
            .optional()
    }

    /// 批量 upsert（单事务）：列表解析的元数据缓存（1500 曲歌单避免逐条提交）。
    /// 不返回 id（缓存场景不需要）；失败整体回滚。
    pub fn upsert_tracks_batch(&mut self, rows: &[TrackRow]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        for row in rows {
            tx.execute(
                r#"INSERT INTO tracks (source, source_key, title, album, artist, duration_ms, cover_uri)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                   ON CONFLICT(source, source_key) DO UPDATE SET
                     title = excluded.title,
                     album = COALESCE(excluded.album, tracks.album),
                     artist = COALESCE(excluded.artist, tracks.artist),
                     duration_ms = COALESCE(excluded.duration_ms, tracks.duration_ms),
                     cover_uri = COALESCE(excluded.cover_uri, tracks.cover_uri)"#,
                params![
                    row.source,
                    row.source_key,
                    row.title,
                    row.album,
                    row.artist,
                    row.duration_ms,
                    row.cover_uri
                ],
            )?;
        }
        tx.commit()
    }

    /// 批量查询曲目元数据（投影层：queue list 等把 ID 列表一次映射成标题/歌手）。
    /// 同一 source 的 key 列表；SQLite 变量上限 999 → 按 500 分片。
    pub fn track_meta_batch(
        &mut self,
        source: &str,
        keys: &[String],
    ) -> rusqlite::Result<Vec<TrackMeta>> {
        let mut out = Vec::with_capacity(keys.len());
        for chunk in keys.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT source, source_key, title, artist, album, duration_ms, cover_uri \
                 FROM tracks WHERE source = ?1 AND source_key IN ({placeholders})"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&source];
            params.extend(chunk.iter().map(|k| k as &dyn rusqlite::ToSql));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
                Ok(TrackMeta {
                    source: r.get(0)?,
                    source_key: r.get(1)?,
                    title: r.get(2)?,
                    artist: r.get(3)?,
                    album: r.get(4)?,
                    duration_ms: r.get(5)?,
                    cover_uri: r.get(6)?,
                })
            })?;
            out.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        Ok(out)
    }

    /// 本地文件入库：upsert tracks(source='local', source_key=`local:<path>`) +
    /// local_files(path 唯一)；返回 track id。
    /// 幂等：同一路径重扫只更新元数据，不重复建曲目。
    pub fn add_local_file(
        &mut self,
        path: &Path,
        meta: Option<&crate::local::LocalMeta>,
    ) -> rusqlite::Result<i64> {
        let meta = match meta {
            Some(m) => m.clone(),
            None => crate::local::LocalMeta {
                title: path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("(unknown title)")
                    .to_string(),
                ..Default::default()
            },
        };
        let source_key = format!("local:{}", path.display());
        // 多艺术家口径与扫描路径一致（先播放后扫描的曲目也能被 artists 聚合命中）。
        let artists: Vec<String> = if meta.artists.is_empty() {
            meta.artist.iter().cloned().collect()
        } else {
            meta.artists.clone()
        };
        let title = if meta.title.trim().is_empty() {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("(unknown title)")
                .to_string()
        } else {
            meta.title
        };
        let id = self.upsert_track(&TrackRow {
            source: "local",
            source_key,
            title,
            album: meta.album,
            artist: meta.artist,
            duration_ms: meta.duration_ms,
            cover_uri: None,
            qq_song_id: None,
            ..Default::default()
        })?;
        if !artists.is_empty() {
            self.write_track_artists(id, &artists)?;
        }
        let md = std::fs::metadata(path).ok();
        self.conn.execute(
            r#"INSERT INTO local_files (track_id, path, file_size, mtime, format, bitrate, sample_rate)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT(path) DO UPDATE SET
                 track_id = excluded.track_id,
                 file_size = excluded.file_size,
                 mtime = excluded.mtime,
                 format = COALESCE(excluded.format, local_files.format),
                 bitrate = COALESCE(excluded.bitrate, local_files.bitrate),
                 sample_rate = COALESCE(excluded.sample_rate, local_files.sample_rate),
                 missing = 0
               ON CONFLICT(track_id) DO UPDATE SET
                 path = excluded.path,
                 file_size = excluded.file_size,
                 mtime = excluded.mtime,
                 format = COALESCE(excluded.format, local_files.format),
                 bitrate = COALESCE(excluded.bitrate, local_files.bitrate),
                 sample_rate = COALESCE(excluded.sample_rate, local_files.sample_rate),
                 missing = 0"#,
            params![
                id,
                path.display().to_string(),
                md.as_ref().map(|m| m.len() as i64),
                md.as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64),
                meta.format,
                meta.bitrate,
                meta.sample_rate,
            ],
        )?;
        Ok(id)
    }

    /// 本地曲目路径（local_files 关联）。
    pub fn local_path(&mut self, track_id: i64) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT path FROM local_files WHERE track_id = ?1",
                params![track_id],
                |r| r.get(0),
            )
            .optional()
    }

    /// 注册扫描根并推进 generation，返回 (root_id, generation)。
    /// 首次扫描 generation=1，之后每次 +1（增量/缺失判定的代际基准）。
    pub fn begin_scan(&mut self, root: &Path) -> rusqlite::Result<(i64, i64)> {
        let canonical = crate::canonical_display_path(root);
        let path_str = canonical.display().to_string();
        self.conn.execute(
            "INSERT INTO scan_roots (path, generation) VALUES (?1, 1)
             ON CONFLICT(path) DO UPDATE SET generation = generation + 1",
            params![path_str],
        )?;
        self.conn.query_row(
            "SELECT id, generation FROM scan_roots WHERE path = ?1",
            params![path_str],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    /// 记录一个扫描文件：新增/更新/跳过/复位。
    /// 增量判定：同 path 且 mtime_ns+size 一致 → Skipped（仍刷新代际、清 missing）；
    /// path 不存在但指纹命中 → 复用行更新路径（移动/改名）。
    pub fn record_scan_file(
        &mut self,
        root_id: i64,
        generation: i64,
        path: &Path,
        meta: Option<&crate::local::LocalMeta>,
        fingerprint: &str,
    ) -> rusqlite::Result<ScanOutcome> {
        // 单事务：新增/更新/复用路径均多段语句，中途失败不留孤儿行（Review）。
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<ScanOutcome> {
            let path_str = path.display().to_string();
            let md = std::fs::metadata(path).ok();
            let mtime_ns = md
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64);
            let size = md.as_ref().map(|m| m.len() as i64);

            let existing: Option<(i64, Option<i64>, Option<i64>, i64)> = self
            .conn
            .query_row(
                "SELECT track_id, mtime_ns, file_size, missing FROM local_files WHERE path = ?1",
                params![path_str],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;

            if let Some((track_id, old_ns, old_size, missing)) = existing {
                if old_ns == mtime_ns && old_size == size {
                    // 增量跳过：刷新代际 + 清 missing（若曾缺失）。
                    self.conn.execute(
                    "UPDATE local_files SET last_seen_generation = ?1, missing = 0 WHERE track_id = ?2",
                    params![generation, track_id],
                )?;
                    return Ok(if missing == 1 {
                        ScanOutcome::MissingReset
                    } else {
                        ScanOutcome::Skipped
                    });
                }
                // 变化：更新元数据 + 文件行。
                if let Some(m) = meta {
                    self.apply_local_meta(track_id, m)?;
                }
                self.conn.execute(
                    "UPDATE local_files SET file_size = ?1, mtime = ?2, mtime_ns = ?3,
                        format = COALESCE(?4, format), bitrate = COALESCE(?5, bitrate),
                        sample_rate = COALESCE(?6, sample_rate), fingerprint = ?7,
                        last_seen_generation = ?8, missing = 0, scan_root_id = ?9
                 WHERE track_id = ?10",
                    params![
                        size,
                        mtime_ns.map(|n| n / 1_000_000_000),
                        mtime_ns,
                        meta.and_then(|m| m.format.clone()),
                        meta.and_then(|m| m.bitrate),
                        meta.and_then(|m| m.sample_rate),
                        fingerprint,
                        generation,
                        root_id,
                        track_id
                    ],
                )?;
                return Ok(ScanOutcome::Updated);
            }

            // path 不存在：指纹命中 → 复用行（移动/改名；指纹含内容+size，命中后
            // 再校验 mtime 一致——内容相同但写入时刻不同的文件不复用）。
            if let Some((tid, _orig)) = self.find_by_fingerprint(fingerprint)? {
                let row_mtime: Option<Option<i64>> = self
                    .conn
                    .query_row(
                        "SELECT mtime_ns FROM local_files WHERE track_id = ?1",
                        params![tid],
                        |r| r.get(0),
                    )
                    .optional()?;
                if row_mtime.flatten() == mtime_ns {
                    // 新路径的 tracks 行已存在（两份内容相同的文件，如 `cp -p`
                    // 或同一压缩包解出两份）：不做移动复用——直接走下方全新
                    // 插入。否则 UPDATE tracks.source_key 会撞
                    // UNIQUE(source, source_key) 中止整个扫描，且两份文件会
                    // 来回「偷」同一行（各自扫描时互相改写路径）。
                    let key_free: bool = self.conn.query_row(
                        "SELECT NOT EXISTS(SELECT 1 FROM tracks WHERE source = 'local' AND source_key = ?1)",
                        params![format!("local:{path_str}")],
                        |r| r.get(0),
                    )?;
                    if key_free {
                        self.conn.execute(
                        "UPDATE local_files SET path = ?1, file_size = ?2, mtime = ?3, mtime_ns = ?4,
                                fingerprint = ?5, last_seen_generation = ?6, missing = 0, scan_root_id = ?7
                         WHERE track_id = ?8",
                        params![
                            path_str,
                            size,
                            mtime_ns.map(|n| n / 1_000_000_000),
                            mtime_ns,
                            fingerprint,
                            generation,
                            root_id,
                            tid
                        ],
                    )?;
                        // 同步 tracks 身份：`local:<旧路径>` → `local:<新路径>`（播放/查询用）。
                        self.conn.execute(
                            "UPDATE tracks SET source_key = ?1 WHERE id = ?2",
                            params![format!("local:{path_str}"), tid],
                        )?;
                        if let Some(m) = meta {
                            self.apply_local_meta(tid, m)?;
                        }
                        return Ok(ScanOutcome::Updated);
                    }
                }
            }

            // 全新：upsert track + local_files + track_artists。
            let meta_owned = meta.cloned();
            let title = meta_owned
                .as_ref()
                .map(|m| m.title.clone())
                .filter(|t| !t.trim().is_empty()) // 无标签文件的空标题 → 文件名回退
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("(unknown title)")
                        .to_string()
                });
            let tid = self.upsert_track(&TrackRow {
                source: "local",
                source_key: format!("local:{path_str}"),
                title,
                album: meta_owned.as_ref().and_then(|m| m.album.clone()),
                artist: meta_owned.as_ref().and_then(|m| m.artist.clone()),
                duration_ms: meta_owned.as_ref().and_then(|m| m.duration_ms),
                cover_uri: None,
                qq_song_id: None,
                album_artist: meta_owned.as_ref().and_then(|m| m.album_artist.clone()),
                track_number: meta_owned
                    .as_ref()
                    .and_then(|m| m.track_number.map(|n| n as i64)),
                disc_number: meta_owned
                    .as_ref()
                    .and_then(|m| m.disc_number.map(|n| n as i64)),
                year: meta_owned.as_ref().and_then(|m| m.year),
                genre: meta_owned.as_ref().and_then(|m| m.genre.clone()),
            })?;
            self.conn.execute(
            r#"INSERT INTO local_files (track_id, path, file_size, mtime, mtime_ns, format, bitrate, sample_rate, fingerprint, last_seen_generation, missing, scan_root_id)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0,?11)"#,
            params![
                tid,
                path_str,
                size,
                mtime_ns.map(|n| n / 1_000_000_000),
                mtime_ns,
                meta_owned.as_ref().and_then(|m| m.format.clone()),
                meta_owned.as_ref().and_then(|m| m.bitrate),
                meta_owned.as_ref().and_then(|m| m.sample_rate),
                fingerprint,
                generation,
                root_id
            ],
        )?;
            if let Some(m) = &meta_owned {
                self.write_track_artists(tid, &m.artists)?;
            }
            Ok(ScanOutcome::Added)
        })();
        match result {
            Ok(out) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(out)
            }
            Err(err) => {
                self.conn.execute_batch("ROLLBACK").ok();
                Err(err)
            }
        }
    }

    /// 扫描收尾：本代际未见到的文件标 missing（不删行；返回新标记数）。
    pub fn finish_scan(&mut self, root_id: i64, generation: i64) -> rusqlite::Result<u32> {
        let n = self.conn.execute(
            "UPDATE local_files SET missing = 1
             WHERE scan_root_id = ?1 AND last_seen_generation < ?2 AND missing = 0",
            params![root_id, generation],
        )?;
        Ok(n as u32)
    }

    /// 清除缺失标记（文件重新出现/手动确认）。
    pub fn clear_missing(&mut self, track_id: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE local_files SET missing = 0 WHERE track_id = ?1",
            params![track_id],
        )?;
        Ok(())
    }

    /// 单文件删除标记（watcher Remove 事件；不删行，missing 语义与扫描一致）。
    /// 返回改动行数（0 = 路径不在库中）。
    pub fn mark_missing_by_path(&mut self, path: &Path) -> rusqlite::Result<u32> {
        let n = self.conn.execute(
            "UPDATE local_files SET missing = 1 WHERE path = ?1 AND missing = 0",
            params![path.display().to_string()],
        )?;
        Ok(n as u32)
    }

    /// 按指纹查找候选行（移动/改名复用）；返回 (track_id, 原 path)。
    pub fn find_by_fingerprint(&mut self, fp: &str) -> rusqlite::Result<Option<(i64, String)>> {
        self.conn
            .query_row(
                "SELECT track_id, path FROM local_files WHERE fingerprint = ?1 LIMIT 1",
                params![fp],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    /// 更新本地曲目封面 URI（仅不同才写；scan 封面提取后调用）。
    pub fn set_track_cover(&mut self, source_key: &str, cover_uri: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE tracks SET cover_uri = ?2
             WHERE source = 'local' AND source_key = ?1 AND COALESCE(cover_uri, '') <> ?2",
            params![source_key, cover_uri],
        )?;
        Ok(())
    }

    /// 远程封面 URL → 本地产物 URI 回写（daemon `CoverGet` 下载成功后调用）：
    /// 播放解析落库的是远程 https URL，UI 禁直连 HTTP，列表/队列
    /// 投影从此读盘渲染。按 URL 匹配（同封面多曲一改全改）；目标已相同
    /// 时零行写入（幂等，重复下载不产生写放大）。不改媒体库代际——封面
    /// 补齐不触发整页重查，UI 队列行由桌面端取图回包原地更新。
    pub fn rebind_cover_url(
        &mut self,
        remote_url: &str,
        local_uri: &str,
    ) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE tracks SET cover_uri = ?2
             WHERE cover_uri = ?1 AND cover_uri <> ?2",
            params![remote_url, local_uri],
        )
    }

    /// 歌单封面回写（reconcile 取得本地产物后调用；仅未设置时写入——
    /// 已有封面不回退，远端换封面待后续轮次再议）。
    pub fn set_playlist_cover(
        &mut self,
        remote_id: &str,
        cover_uri: &str,
    ) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE playlists SET cover_uri = ?2
             WHERE provider = 'qq' AND remote_id = ?1 AND (cover_uri IS NULL OR cover_uri = '')",
            params![remote_id, cover_uri],
        )
    }

    /// 尚无封面的 QQ 歌单（remote_id 列表；封面补抓的输入）。
    pub fn qq_playlists_missing_cover(&mut self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT remote_id FROM playlists WHERE provider = 'qq' AND remote_id IS NOT NULL AND (cover_uri IS NULL OR cover_uri = '')")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    /// 标题仍为 mid 的 QQ 曲（stub 元数据修复的输入；逐批限额）。
    pub fn qq_stub_title_keys(&mut self, limit: i64) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_key FROM tracks WHERE source = 'qq' AND title = source_key LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    /// 写入完整元数据（tracks 行 + track_artists 重写）。
    fn apply_local_meta(
        &mut self,
        track_id: i64,
        m: &crate::local::LocalMeta,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE tracks SET title = ?1, album = ?2, artist = ?3, duration_ms = ?4,
                    album_artist = ?5, track_number = ?6, disc_number = ?7, year = ?8, genre = ?9
             WHERE id = ?10",
            params![
                m.title,
                m.album,
                m.artist,
                m.duration_ms,
                m.album_artist,
                m.track_number.map(|n| n as i64),
                m.disc_number.map(|n| n as i64),
                m.year,
                m.genre,
                track_id
            ],
        )?;
        self.write_track_artists(track_id, &m.artists)
    }

    /// 重写多艺术家行（先删后插，position 保序）。
    fn write_track_artists(&mut self, track_id: i64, artists: &[String]) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM track_artists WHERE track_id = ?1",
            params![track_id],
        )?;
        for (i, a) in artists.iter().enumerate() {
            self.conn.execute(
                "INSERT OR IGNORE INTO track_artists (track_id, artist, position) VALUES (?1,?2,?3)",
                params![track_id, a, i as i64],
            )?;
        }
        Ok(())
    }

    /// LIKE 通配符转义（配合 `ESCAPE '\\'`）。
    fn escape_like(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    }

    /// 本地曲目浏览：search（标题/歌手/专辑子串）、artist（track_artists 多值命中）、
    /// album（精确）、liked_only（relations 收藏）。
    pub fn library_tracks(
        &mut self,
        search: Option<&str>,
        artist: Option<&str>,
        album: Option<&str>,
        liked_only: bool,
    ) -> rusqlite::Result<Vec<LibraryTrackRow>> {
        let mut sql = String::from(
            "SELECT t.id, t.source_key, t.title, t.artist, t.album, t.duration_ms, t.year, t.genre, lf.missing
             FROM tracks t JOIN local_files lf ON lf.track_id = t.id WHERE t.source = 'local'",
        );
        let mut conds: Vec<String> = Vec::new();
        let mut vals: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(s) = search {
            conds.push(
                "(t.title LIKE ? ESCAPE '\\' OR COALESCE(t.artist,'') LIKE ? ESCAPE '\\' OR COALESCE(t.album,'') LIKE ? ESCAPE '\\')".into(),
            );
            let pat = format!("%{}%", Self::escape_like(s));
            for _ in 0..3 {
                vals.push(Box::new(pat.clone()));
            }
        }
        if let Some(a) = artist {
            conds.push(
                "EXISTS (SELECT 1 FROM track_artists ta WHERE ta.track_id = t.id AND ta.artist = ?)"
                    .into(),
            );
            vals.push(Box::new(a.to_string()));
        }
        if let Some(al) = album {
            conds.push("t.album = ? COLLATE NOCASE".into());
            vals.push(Box::new(al.to_string()));
        }
        if liked_only {
            conds.push(
                "EXISTS (SELECT 1 FROM relations r WHERE r.entity_type='track' AND r.provider='local' AND r.entity_key = t.source_key AND r.relation='liked' AND r.desired_state=1)"
                    .into(),
            );
        }
        if !conds.is_empty() {
            sql.push_str(" AND ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" ORDER BY t.title");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(vals.iter().map(|v| v.as_ref())),
            |r| {
                Ok(LibraryTrackRow {
                    track_id: r.get(0)?,
                    source_key: r.get(1)?,
                    title: r.get(2)?,
                    artist: r.get(3)?,
                    album: r.get(4)?,
                    duration_ms: r.get(5)?,
                    year: r.get(6)?,
                    genre: r.get(7)?,
                    missing: r.get::<_, i64>(8)? != 0,
                })
            },
        )?;
        rows.collect()
    }

    /// 本地专辑聚合（album 非空；search 子串过滤）。
    pub fn library_albums(&mut self, search: Option<&str>) -> rusqlite::Result<Vec<AlbumGroup>> {
        let mut sql = String::from(
            "SELECT t.album, MAX(t.artist), COUNT(*), MAX(t.year), MAX(t.cover_uri)
             FROM tracks t JOIN local_files lf ON lf.track_id = t.id
             WHERE t.source = 'local' AND t.album IS NOT NULL AND t.album <> ''",
        );
        let mut vals: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(s) = search {
            sql.push_str(" AND t.album LIKE ? ESCAPE '\\'");
            vals.push(Box::new(format!("%{}%", Self::escape_like(s))));
        }
        sql.push_str(" GROUP BY t.album COLLATE NOCASE ORDER BY t.album COLLATE NOCASE");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(vals.iter().map(|v| v.as_ref())),
            |r| {
                Ok(AlbumGroup {
                    album: r.get(0)?,
                    artist: r.get(1)?,
                    track_count: r.get(2)?,
                    year: r.get(3)?,
                    cover_uri: r.get(4)?,
                })
            },
        )?;
        rows.collect()
    }

    /// 本地歌手聚合（多值拆行；曲目数按 distinct track 计）。
    pub fn library_artists(&mut self) -> rusqlite::Result<Vec<ArtistGroup>> {
        let mut stmt = self.conn.prepare(
            "SELECT ta.artist, COUNT(DISTINCT ta.track_id)
             FROM track_artists ta
             JOIN tracks t ON t.id = ta.track_id
             JOIN local_files lf ON lf.track_id = t.id
             WHERE t.source = 'local'
             GROUP BY ta.artist ORDER BY ta.artist",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ArtistGroup {
                artist: r.get(0)?,
                track_count: r.get(1)?,
            })
        })?;
        rows.collect()
    }

    /// 按专辑名取本地曲目（播放源；大小写不敏感精确匹配）。
    pub fn local_tracks_by_album(&mut self, album: &str) -> rusqlite::Result<Vec<LibraryTrackRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.source_key, t.title, t.artist, t.album, t.duration_ms, t.year, t.genre, lf.missing
             FROM tracks t JOIN local_files lf ON lf.track_id = t.id
             WHERE t.source = 'local' AND t.album = ?1 COLLATE NOCASE
             ORDER BY COALESCE(t.track_number, 999), t.title",
        )?;
        let rows = stmt.query_map(params![album], |r| {
            Ok(LibraryTrackRow {
                track_id: r.get(0)?,
                source_key: r.get(1)?,
                title: r.get(2)?,
                artist: r.get(3)?,
                album: r.get(4)?,
                duration_ms: r.get(5)?,
                year: r.get(6)?,
                genre: r.get(7)?,
                missing: r.get::<_, i64>(8)? != 0,
            })
        })?;
        rows.collect()
    }

    /// 按歌手名取本地曲目（播放源；track_artists 多值命中）。
    pub fn local_tracks_by_artist(
        &mut self,
        artist: &str,
    ) -> rusqlite::Result<Vec<LibraryTrackRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.id, t.source_key, t.title, t.artist, t.album, t.duration_ms, t.year, t.genre, lf.missing
             FROM tracks t
             JOIN track_artists ta ON ta.track_id = t.id
             JOIN local_files lf ON lf.track_id = t.id
             WHERE t.source = 'local' AND ta.artist = ?1
             ORDER BY t.title",
        )?;
        let rows = stmt.query_map(params![artist], |r| {
            Ok(LibraryTrackRow {
                track_id: r.get(0)?,
                source_key: r.get(1)?,
                title: r.get(2)?,
                artist: r.get(3)?,
                album: r.get(4)?,
                duration_ms: r.get(5)?,
                year: r.get(6)?,
                genre: r.get(7)?,
                missing: r.get::<_, i64>(8)? != 0,
            })
        })?;
        rows.collect()
    }

    /// 已注册扫描根（watcher/E2 用）。
    pub fn scan_roots(&mut self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM scan_roots ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
    }

    /// 路径所属扫描根（canonical 前缀匹配）→ (root_id, 当前 generation)。
    /// 供 watcher 事件处理：单文件入库用 root 当前代际，不推进 generation。
    pub fn scan_root_for(&mut self, path: &Path) -> rusqlite::Result<Option<(i64, i64)>> {
        let canonical = crate::canonical_display_path(path);
        let roots: Vec<(i64, String, i64)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, path, generation FROM scan_roots")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        Ok(roots
            .into_iter()
            .filter(|(_, root, _)| canonical.starts_with(std::path::Path::new(root)))
            // 最长前缀：嵌套扫描根（/music 与 /music/inner）取最内层。
            .max_by_key(|(_, root, _)| root.len())
            .map(|(id, _, generation)| (id, generation)))
    }

    /// 收藏曲目（upsert 曲目行 + 收藏；幂等）。
    /// `source`/`source_key` 与播放历史一致（qq → mid；local → `local:<path>`）。
    /// 收藏曲目（本地先提交：upsert 曲目行 + relations(track,liked,desired=true)）。
    /// 幂等；QQ 同步由 daemon SyncWorker 消费 outbox。
    pub fn add_favorite(
        &mut self,
        source: &'static str,
        source_key: &str,
        title: &str,
    ) -> rusqlite::Result<i64> {
        let tid = self.upsert_track(&TrackRow {
            source,
            source_key: source_key.to_owned(),
            title: title.to_owned(),
            album: None,
            artist: None,
            duration_ms: None,
            cover_uri: None,
            qq_song_id: None,
            ..Default::default()
        })?;
        self.set_relation("track", source, source_key, "liked", true)?;
        Ok(tid)
    }

    /// 取消收藏（本地先提交：desired=false，留 outbox 待同步 unlike）。
    pub fn remove_favorite(&mut self, track_id: i64) -> rusqlite::Result<()> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT source, source_key FROM tracks WHERE id = ?1",
                params![track_id],
                |r| -> rusqlite::Result<(String, String)> { Ok((r.get(0)?, r.get(1)?)) },
            )
            .optional()?;
        if let Some((source, key)) = row {
            self.set_relation("track", &source, &key, "liked", false)?;
        }
        Ok(())
    }

    /// 是否已收藏（本地事实视图：desired=true 即视为已收藏）。
    pub fn is_favorite(&mut self, track_id: i64) -> rusqlite::Result<bool> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT source, source_key FROM tracks WHERE id = ?1",
                params![track_id],
                |r| -> rusqlite::Result<(String, String)> { Ok((r.get(0)?, r.get(1)?)) },
            )
            .optional()?;
        match row {
            Some((source, key)) => Ok(self
                .relation_desired("track", &source, &key, "liked")?
                .unwrap_or(false)),
            None => Ok(false),
        }
    }

    /// 收藏列表（本地事实视图，新→旧）。
    pub fn list_favorites(&mut self, limit: u32) -> rusqlite::Result<Vec<FavoriteRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.source, t.source_key, t.title, r.updated_at
             FROM relations r JOIN tracks t
               ON t.source = r.provider AND t.source_key = r.entity_key
             WHERE r.entity_type = 'track' AND r.relation = 'liked' AND r.desired_state = 1
             ORDER BY r.updated_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(FavoriteRow {
                track_id: r.get(0)?,
                source: r.get(1)?,
                source_key: r.get(2)?,
                title: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        rows.collect()
    }

    // ---- 关系表（relations = durable outbox 一体，媒体库内核 spec §3.1）----

    /// 写入本地意图（操作合并：同 PK upsert 覆盖，最后一次意图胜出）。
    /// 已 synced 且意图与远端一致时不置 pending（省一次远端请求）。
    pub fn set_relation(
        &mut self,
        entity_type: &str,
        provider: &str,
        entity_key: &str,
        relation: &str,
        desired: bool,
    ) -> rusqlite::Result<()> {
        let entity_key = canonical_local_key(provider, entity_key);
        let now = now_unix();
        let desired = i64::from(desired);
        // 已同步且与远端一致 → 仅刷新时间戳，不进 outbox。
        let settled: Option<i64> = self
            .conn
            .query_row(
                "SELECT last_remote_state FROM relations WHERE entity_type=?1 AND provider=?2 \
                 AND entity_key=?3 AND relation=?4 AND sync_state='synced'",
                params![entity_type, provider, entity_key, relation],
                |r| r.get(0),
            )
            .optional()?;
        if settled == Some(desired) {
            self.conn.execute(
                "UPDATE relations SET updated_at=?1 WHERE entity_type=?2 AND provider=?3 \
                 AND entity_key=?4 AND relation=?5",
                params![now, entity_type, provider, entity_key, relation],
            )?;
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO relations (entity_type, provider, entity_key, relation, \
             desired_state, sync_state, updated_at) VALUES (?1,?2,?3,?4,?5,'pending',?6) \
             ON CONFLICT(entity_type, provider, entity_key, relation) DO UPDATE SET \
               desired_state = excluded.desired_state, sync_state = 'pending', \
               retry_count = 0, last_sync_error = NULL, updated_at = excluded.updated_at",
            params![entity_type, provider, entity_key, relation, desired, now],
        )?;
        Ok(())
    }

    /// 本地意图查询（无行 → None）。
    pub fn relation_desired(
        &mut self,
        entity_type: &str,
        provider: &str,
        entity_key: &str,
        relation: &str,
    ) -> rusqlite::Result<Option<bool>> {
        let entity_key = canonical_local_key(provider, entity_key);
        let v: Option<i64> = self
            .conn
            .query_row(
                "SELECT desired_state FROM relations WHERE entity_type=?1 AND provider=?2 \
                 AND entity_key=?3 AND relation=?4",
                params![entity_type, provider, entity_key, relation],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.map(|x| x != 0))
    }

    /// outbox 扫描：待同步/重试的关系行（错误优先重试？按 updated_at 升序）。
    pub fn relations_pending(&mut self) -> rusqlite::Result<Vec<RelationRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT entity_type, provider, entity_key, relation, desired_state, \
             last_remote_state, sync_state, retry_count, last_sync_error, updated_at \
             FROM relations WHERE sync_state != 'synced' ORDER BY updated_at",
        )?;
        let rows = stmt.query_map([], row_of_relation)?;
        rows.collect()
    }

    /// 同步成功：置 synced + 远端状态 = 本地意图。
    pub fn mark_relation_synced(
        &mut self,
        entity_type: &str,
        provider: &str,
        entity_key: &str,
        relation: &str,
    ) -> rusqlite::Result<()> {
        let entity_key = canonical_local_key(provider, entity_key);
        self.conn.execute(
            "UPDATE relations SET sync_state='synced', retry_count=0, last_sync_error=NULL, \
             last_remote_state=desired_state, updated_at=?1 \
             WHERE entity_type=?2 AND provider=?3 AND entity_key=?4 AND relation=?5",
            params![now_unix(), entity_type, provider, entity_key, relation],
        )?;
        Ok(())
    }

    /// 同步失败：置 error + 重试计数（SyncWorker 指数退避）。
    pub fn mark_relation_error(
        &mut self,
        entity_type: &str,
        provider: &str,
        entity_key: &str,
        relation: &str,
        err: &str,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE relations SET sync_state='error', retry_count=retry_count+1, \
             last_sync_error=?1, updated_at=?2 \
             WHERE entity_type=?3 AND provider=?4 AND entity_key=?5 AND relation=?6",
            params![err, now_unix(), entity_type, provider, entity_key, relation],
        )?;
        Ok(())
    }

    /// reconcile：写入远端事实（无 pending 意图时 QQ snapshot 胜，spec §3.1）。
    /// 存在 pending 本地意图 → 跳过（本地胜）。
    pub fn reconcile_relation(
        &mut self,
        entity_type: &str,
        provider: &str,
        entity_key: &str,
        relation: &str,
        remote_state: bool,
    ) -> rusqlite::Result<()> {
        let now = now_unix();
        let remote = i64::from(remote_state);
        let pending: Option<i64> = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM relations WHERE entity_type=?1 AND provider=?2 \
                 AND entity_key=?3 AND relation=?4 AND sync_state != 'synced'",
                params![entity_type, provider, entity_key, relation],
                |r| r.get(0),
            )
            .optional()?;
        if pending.unwrap_or(0) > 0 {
            return Ok(()); // 本地意图优先：跳过
        }
        self.conn.execute(
            "INSERT INTO relations (entity_type, provider, entity_key, relation, \
             desired_state, last_remote_state, sync_state, updated_at) \
             VALUES (?1,?2,?3,?4,?5,?5,'synced',?6) \
             ON CONFLICT(entity_type, provider, entity_key, relation) DO UPDATE SET \
               desired_state = excluded.desired_state, \
               last_remote_state = excluded.last_remote_state, \
               sync_state = 'synced', retry_count = 0, last_sync_error = NULL, \
               updated_at = excluded.updated_at",
            params![entity_type, provider, entity_key, relation, remote, now],
        )?;
        Ok(())
    }

    /// reconcile：按远端身份 upsert 歌单（owned=dirid / subscribed=disstid）。
    /// reconcile：远端缺席的行（本地 desired=1 已 synced，但远端快照无此实体）
    /// → 置 desired=0（QQ snapshot 胜；pending 行不受影响）。分片避开 999 变量上限。
    /// `provider` 限定远端源（qq），本地（local）收藏不受 QQ 快照影响。
    /// `present_keys` 为空（远端快照真 0 条）→ 全量清理该 provider 的 synced 行。
    pub fn reconcile_remove_absent(
        &mut self,
        entity_type: &str,
        provider: &str,
        relation: &str,
        present_keys: &[String],
    ) -> rusqlite::Result<()> {
        let now = now_unix();
        if present_keys.is_empty() {
            // 远端快照为 0 条：全量清理该 provider 的 synced 行。
            self.conn.execute(
                "UPDATE relations SET desired_state=0, last_remote_state=0, \
                 sync_state='synced', retry_count=0, last_sync_error=NULL, updated_at=?1 \
                 WHERE entity_type=?2 AND provider=?3 AND relation=?4 AND desired_state=1 \
                 AND sync_state='synced'",
                rusqlite::params![now, entity_type, provider, relation],
            )?;
            return Ok(());
        }
        for chunk in present_keys.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "UPDATE relations SET desired_state=0, last_remote_state=0, \
                 sync_state='synced', retry_count=0, last_sync_error=NULL, updated_at=?1 \
                 WHERE entity_type=?2 AND provider=?3 AND relation=?4 AND desired_state=1 \
                 AND sync_state='synced' AND entity_key NOT IN ({placeholders})"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> =
                vec![&now, &entity_type, &provider, &relation];
            params.extend(chunk.iter().map(|k| k as &dyn rusqlite::ToSql));
            self.conn
                .execute(&sql, rusqlite::params_from_iter(params))?;
        }
        Ok(())
    }

    /// reconcile：远端缺席的歌单（relation=subscribed 已 synced 但远端快照无此 disstid）
    /// → 删除本地行（与 server 取消收藏行为一致，不留幽灵条目）。
    /// `present_keys` 为空 → 全量清理。
    pub fn delete_playlists_absent(
        &mut self,
        relation: &str,
        present_keys: &[String],
    ) -> rusqlite::Result<()> {
        if present_keys.is_empty() {
            self.conn.execute(
                "DELETE FROM playlists WHERE relation=?1 AND sync_state='synced'",
                rusqlite::params![relation],
            )?;
            return Ok(());
        }
        for chunk in present_keys.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "DELETE FROM playlists WHERE relation=?1 AND sync_state='synced' \
                 AND remote_id NOT IN ({placeholders})"
            );
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&relation];
            params.extend(chunk.iter().map(|k| k as &dyn rusqlite::ToSql));
            self.conn
                .execute(&sql, rusqlite::params_from_iter(params))?;
        }
        Ok(())
    }

    pub fn reconcile_playlist(
        &mut self,
        remote_id: &str,
        name: &str,
        relation: &str,
    ) -> rusqlite::Result<i64> {
        let now = now_unix();
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM playlists WHERE remote_id = ?1 AND relation = ?2",
                params![remote_id, relation],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            self.conn.execute(
                "UPDATE playlists SET name = ?1, provider = 'qq', sync_state = 'synced', \
                 updated_at = ?2 WHERE id = ?3",
                params![name, now, id],
            )?;
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO playlists (name, created_at, updated_at, provider, remote_id, relation) \
             VALUES (?1, ?2, ?2, 'qq', ?3, ?4)",
            params![name, now, remote_id, relation],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 关系快照（本地事实视图，如 tracks --liked / albums --liked）。
    pub fn relation_rows(
        &mut self,
        entity_type: &str,
        relation: &str,
    ) -> rusqlite::Result<Vec<RelationRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT entity_type, provider, entity_key, relation, desired_state, \
             last_remote_state, sync_state, retry_count, last_sync_error, updated_at \
             FROM relations WHERE entity_type=?1 AND relation=?2 AND desired_state=1 \
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![entity_type, relation], row_of_relation)?;
        rows.collect()
    }

    /// QQ numeric song id 写入（comment biz_id 映射；列表解析批量缓存时带入）。
    pub fn set_track_qq_song_id(
        &mut self,
        source: &str,
        source_key: &str,
        song_id: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE tracks SET qq_song_id=?1 WHERE source=?2 AND source_key=?3",
            params![song_id, source, source_key],
        )?;
        Ok(())
    }

    /// 按 source_key 查 QQ numeric song id（comment biz_id）。
    pub fn qq_song_id(&mut self, source: &str, source_key: &str) -> rusqlite::Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT qq_song_id FROM tracks WHERE source=?1 AND source_key=?2",
                params![source, source_key],
                |r| r.get(0),
            )
            .optional()
            .map(|v: Option<Option<i64>>| v.flatten())
    }

    // ---- 歌单同步（owned：远端身份 + 曲目操作 outbox）----

    /// 歌单置为待同步（owned 删除意图：行保留到远端删除成功）。
    pub fn mark_playlist_pending(&mut self, id: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE playlists SET sync_state='pending', updated_at=?1 WHERE id=?2",
            params![now_unix(), id],
        )?;
        Ok(())
    }

    /// 歌单内指定位置的曲目 source_key（owned 移除操作的 outbox 需要）。
    pub fn track_key_at(
        &mut self,
        playlist_id: i64,
        position: i64,
    ) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT t.source_key FROM playlist_tracks pt JOIN tracks t ON t.id = pt.track_id \
                 WHERE pt.playlist_id=?1 AND pt.position=?2",
                params![playlist_id, position],
                |r| r.get(0),
            )
            .optional()
    }

    /// 歌单远端身份（owned=dirid / subscribed=disstid；local → None）。
    pub fn playlist_remote_id(&mut self, id: i64) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT remote_id FROM playlists WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map(|v: Option<Option<String>>| v.flatten())
    }

    /// 歌单归属（local | owned | subscribed）。
    pub fn playlist_relation(&mut self, id: i64) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT relation FROM playlists WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
    }

    /// 记录远端身份（reconcile/创建成功后；owned=dirid，subscribed=disstid）。
    pub fn set_playlist_remote(
        &mut self,
        id: i64,
        remote_id: &str,
        relation: &str,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE playlists SET remote_id=?1, relation=?2, sync_state='synced', \
             retry_count=0, last_sync_error=NULL, updated_at=?3 WHERE id=?4",
            params![remote_id, relation, now_unix(), id],
        )?;
        Ok(())
    }

    /// 待同步歌单（创建/改名/删除意图；本地歌单不出现）。
    pub fn playlists_pending(&mut self) -> rusqlite::Result<Vec<PlaylistRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, p.created_at, p.provider, p.remote_id, p.relation, \
             p.sync_state, p.retry_count, p.last_sync_error, p.updated_at, p.cover_uri,\n                    (SELECT COUNT(*) FROM playlist_tracks pt WHERE pt.playlist_id = p.id) \
             FROM playlists p WHERE p.relation != 'local' AND p.sync_state != 'synced' \
             ORDER BY p.updated_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PlaylistRow {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                provider: r.get(3)?,
                remote_id: r.get(4)?,
                relation: r.get(5)?,
                sync_state: r.get(6)?,
                retry_count: r.get(7)?,
                last_sync_error: r.get(8)?,
                updated_at: r.get(9)?,
                cover_uri: r.get(10)?,
                track_count: r.get(11)?,
            })
        })?;
        rows.collect()
    }

    /// 歌单同步成功。
    pub fn mark_playlist_synced(&mut self, id: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE playlists SET sync_state='synced', retry_count=0, last_sync_error=NULL, \
             updated_at=?1 WHERE id=?2",
            params![now_unix(), id],
        )?;
        Ok(())
    }

    /// 歌单同步失败。
    pub fn mark_playlist_error(&mut self, id: i64, err: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE playlists SET sync_state='error', retry_count=retry_count+1, \
             last_sync_error=?1, updated_at=?2 WHERE id=?3",
            params![err, now_unix(), id],
        )?;
        Ok(())
    }

    /// owned 歌单曲目操作入 outbox（本地提交后异步同步）。
    /// `song_key` 为 QQ mid（song_id 未知时由 SyncWorker 详情补全）。
    pub fn enqueue_playlist_op(
        &mut self,
        playlist_id: i64,
        op: &str,
        song_key: Option<&str>,
        song_id: Option<i64>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO playlist_ops (playlist_id, op, song_key, song_id, updated_at) \
             VALUES (?1,?2,?3,?4,?5)",
            params![playlist_id, op, song_key, song_id, now_unix()],
        )?;
        Ok(())
    }

    /// outbox 扫描：待同步歌单操作。
    pub fn playlist_ops_pending(&mut self) -> rusqlite::Result<Vec<PlaylistOpRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, playlist_id, op, song_key, song_id, sync_state, retry_count, \
             last_error, updated_at \
             FROM playlist_ops WHERE sync_state != 'done' ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PlaylistOpRow {
                id: r.get(0)?,
                playlist_id: r.get(1)?,
                op: r.get(2)?,
                song_key: r.get(3)?,
                song_id: r.get(4)?,
                sync_state: r.get(5)?,
                retry_count: r.get(6)?,
                last_error: r.get(7)?,
                updated_at: r.get(8)?,
            })
        })?;
        rows.collect()
    }

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

    /// 回填 op 行 numeric song id（SyncWorker 详情补全后）。
    pub fn set_op_song_id(&mut self, id: i64, song_id: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE playlist_ops SET song_id=?1 WHERE id=?2",
            params![song_id, id],
        )?;
        Ok(())
    }

    /// 歌单操作完成（删除 outbox 行）。
    pub fn mark_op_done(&mut self, id: i64) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM playlist_ops WHERE id=?1", params![id])?;
        Ok(())
    }

    /// 歌单操作失败（重试计数）。
    pub fn mark_op_error(&mut self, id: i64, err: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE playlist_ops SET sync_state='error', retry_count=retry_count+1, \
             last_error=?1, updated_at=?2 WHERE id=?3",
            params![err, now_unix(), id],
        )?;
        Ok(())
    }

    /// 新建本地歌单，返回 id。
    pub fn create_playlist(&mut self, name: &str) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO playlists (name, created_at, updated_at) VALUES (?1, ?2, ?2)",
            params![name, now_unix()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 重命名歌单；不存在 → QueryReturnedNoRows。
    pub fn rename_playlist(&mut self, id: i64, name: &str) -> rusqlite::Result<()> {
        let n = self.conn.execute(
            "UPDATE playlists SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, now_unix(), id],
        )?;
        if n == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 删除歌单（级联删曲目关联）。
    pub fn delete_playlist(&mut self, id: i64) -> rusqlite::Result<()> {
        // 级联清理：outbox 操作行 + 曲目关联 + 歌单本体（孤儿 op 会永久卡死）。
        self.conn.execute(
            "DELETE FROM playlist_ops WHERE playlist_id = ?1",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
            params![id],
        )?;
        self.conn
            .execute("DELETE FROM playlists WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 歌单列表（含曲目数）。
    pub fn list_playlists(&mut self) -> rusqlite::Result<Vec<PlaylistRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, p.created_at, p.provider, p.remote_id, p.relation, \
             p.sync_state, p.retry_count, p.last_sync_error, p.updated_at, p.cover_uri,\n                    (SELECT COUNT(*) FROM playlist_tracks pt WHERE pt.playlist_id = p.id) \
             FROM playlists p ORDER BY p.created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PlaylistRow {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                provider: r.get(3)?,
                remote_id: r.get(4)?,
                relation: r.get(5)?,
                sync_state: r.get(6)?,
                retry_count: r.get(7)?,
                last_sync_error: r.get(8)?,
                updated_at: r.get(9)?,
                cover_uri: r.get(10)?,
                track_count: r.get(11)?,
            })
        })?;
        rows.collect()
    }

    /// 歌单内曲目（按 position）。
    pub fn playlist_tracks(&mut self, playlist_id: i64) -> rusqlite::Result<Vec<PlaylistTrackRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT pt.position, t.id, t.title, t.source_key
             FROM playlist_tracks pt JOIN tracks t ON t.id = pt.track_id
             WHERE pt.playlist_id = ?1 ORDER BY pt.position",
        )?;
        let rows = stmt.query_map(params![playlist_id], |r| {
            Ok(PlaylistTrackRow {
                position: r.get(0)?,
                track_id: r.get(1)?,
                title: r.get(2)?,
                source_key: r.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// 本地歌单曲目（播放源，里程碑 F）：按 position 排序；JOIN tracks 带完整元数据。
    pub fn local_playlist_stubs(
        &mut self,
        playlist_id: i64,
    ) -> rusqlite::Result<Vec<LocalPlaylistRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.source_key, t.title, t.artist, t.album, t.duration_ms
             FROM playlist_tracks pt JOIN tracks t ON t.id = pt.track_id
             WHERE pt.playlist_id = ?1 ORDER BY pt.position, pt.rowid",
        )?;
        let rows = stmt.query_map(params![playlist_id], |r| {
            Ok(LocalPlaylistRow {
                source_key: r.get(0)?,
                title: r.get(1)?,
                artist: r.get(2)?,
                album: r.get(3)?,
                duration_ms: r.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// 往歌单追加曲目（幂等：同曲不重复；曲目行按需 upsert）。
    /// 歌单不存在 → QueryReturnedNoRows。
    pub fn add_playlist_track(
        &mut self,
        playlist_id: i64,
        source: &'static str,
        source_key: &str,
        title: &str,
    ) -> rusqlite::Result<()> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM playlists WHERE id = ?1)",
            params![playlist_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        let tid = self.upsert_track(&TrackRow {
            source,
            source_key: source_key.to_owned(),
            title: title.to_owned(),
            album: None,
            artist: None,
            duration_ms: None,
            cover_uri: None,
            qq_song_id: None,
            ..Default::default()
        })?;
        let dup: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2)",
            params![playlist_id, tid],
            |r| r.get(0),
        )?;
        if dup {
            return Ok(());
        }
        let max_pos: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) FROM playlist_tracks WHERE playlist_id = ?1",
            params![playlist_id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![playlist_id, tid, max_pos + 1, now_unix()],
        )?;
        Ok(())
    }

    /// 从歌单移除指定 position 的曲目。
    pub fn remove_playlist_track(
        &mut self,
        playlist_id: i64,
        position: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1 AND position = ?2",
            params![playlist_id, position],
        )?;
        Ok(())
    }
}

/// relations 行映射（供 relations_pending / relation_rows 共用）。
fn row_of_relation(r: &rusqlite::Row<'_>) -> rusqlite::Result<RelationRow> {
    Ok(RelationRow {
        entity_type: r.get(0)?,
        provider: r.get(1)?,
        entity_key: r.get(2)?,
        relation: r.get(3)?,
        desired_state: r.get::<_, i64>(4)? != 0,
        last_remote_state: r.get::<_, Option<i64>>(5)?.map(|v| v != 0),
        sync_state: r.get(6)?,
        retry_count: r.get(7)?,
        last_sync_error: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

/// 当前 unix 时间戳（秒）。
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 本地键规范形：`local:<canonical path>`（与扫描器、播放身份同一约定，
/// daemon local.rs P1 同款语义）。请求侧可能给非 canonical 路径（symlink、
/// 旧挂载点拼写），不归一会让同一文件在 tracks 里长出第二条（扫描行 +
/// 意图幽灵行），收藏/歌单/历史全部错位。文件暂不存在（离线盘/先藏后扫）
/// 时保留原样，待文件出现后由写路径与 v3 迁移收敛。
fn canonical_local_key(provider: &str, key: &str) -> String {
    if provider != "local" {
        return key.to_owned();
    }
    match key.strip_prefix("local:") {
        Some(path) => format!(
            "local:{}",
            crate::canonical_display_path(std::path::Path::new(path)).display()
        ),
        None => key.to_owned(),
    }
}

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

/// v4：扫描/播放热路径索引——find_by_fingerprint 在每次扫描/监听事件中
/// 全表扫描 local_files（O(N²)）；playlist_tracks 缺索引使 list_playlists
/// 的每行 COUNT(*) 退化为全扫。
const MIGRATION_V4: &str = r#"
CREATE INDEX IF NOT EXISTS idx_local_files_fingerprint ON local_files(fingerprint);
CREATE INDEX IF NOT EXISTS idx_playlist_tracks_playlist ON playlist_tracks(playlist_id);
"#;

/// v5：合并本地意图幽灵行。写路径归一化（`canonical_local_key`）落地前，
/// 收藏/歌单写曾以非 canonical `local:<path>` upsert 出与扫描行并存的
/// 幽灵 tracks 行（无 local_files、标题=键、无元数据），relations/歌单
/// 链接指向幽灵。本迁移把可 canonical 化的幽灵合入对应扫描行后删除幽灵。
/// 幂等：无幽灵（或路径已不存在/目标扫描行缺失）时零写入。
pub(crate) fn merge_ghost_local_tracks(conn: &Connection) -> rusqlite::Result<()> {
    let ghosts: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT t.id, t.source_key FROM tracks t \
             WHERE t.source = 'local' AND t.source_key LIKE 'local:%' \
             AND NOT EXISTS (SELECT 1 FROM local_files lf WHERE lf.track_id = t.id)",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (gid, key) in ghosts {
        let Some(path) = key.strip_prefix("local:") else {
            continue;
        };
        // canonicalize 失败（离线盘/已删除）→ 无法收敛，保留幽灵等下次扫描。
        let canon = match std::fs::canonicalize(path) {
            Ok(c) => format!("local:{}", crate::strip_verbatim(&c).display()),
            Err(_) => continue,
        };
        if canon == key {
            continue;
        }
        // 目标扫描行必须存在；文件从未正式入库 → 保留幽灵等下次扫描收敛。
        let target: Option<i64> = conn
            .query_row(
                "SELECT t.id FROM tracks t WHERE t.source = 'local' AND t.source_key = ?1 \
                 AND EXISTS (SELECT 1 FROM local_files lf WHERE lf.track_id = t.id)",
                params![canon],
                |r| r.get(0),
            )
            .optional()?;
        let Some(tid) = target else { continue };
        // relations：意图搬到规范键；规范键已有同 relation 行则弃幽灵保既有。
        let rels: Vec<(String, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT relation, desired_state FROM relations \
                 WHERE entity_type = 'track' AND provider = 'local' AND entity_key = ?1",
            )?;
            let rows = stmt.query_map(params![key], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (relation, desired) in rels {
            conn.execute(
                "INSERT INTO relations (entity_type, provider, entity_key, relation, \
                 desired_state, sync_state, last_remote_state, updated_at) \
                 VALUES ('track', 'local', ?1, ?2, ?3, 'synced', ?3, ?4) \
                 ON CONFLICT(entity_type, provider, entity_key, relation) DO NOTHING",
                params![canon, relation, desired, now_unix()],
            )?;
            conn.execute(
                "DELETE FROM relations WHERE entity_type = 'track' AND provider = 'local' \
                 AND entity_key = ?1 AND relation = ?2",
                params![key, relation],
            )?;
        }
        // 歌单链接重指向（表无 (playlist_id, track_id) 唯一约束，直接 UPDATE）。
        conn.execute(
            "UPDATE playlist_tracks SET track_id = ?1 WHERE track_id = ?2",
            params![tid, gid],
        )?;
        conn.execute("DELETE FROM tracks WHERE id = ?1", params![gid])?;
    }
    Ok(())
}

/// 逐级迁移到最新 user_version。
fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current < 1 {
        // v1 迁移同样包事务：中途失败整体回滚（user_version 不推进，库不砖化）。
        conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            conn.execute_batch(SCHEMA_V1)?;
            conn.pragma_update(None, "user_version", 1)?;
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
    if current < 2 {
        // v2 迁移包事务：任一步失败整体回滚（user_version 不推进，
        // 库不砖化——否则中途失败重开库会在 CREATE TABLE 处报已存在）。
        conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            conn.execute_batch(MIGRATION_V2)?;
            conn.pragma_update(None, "user_version", 2)?;
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
    if current < 3 {
        // v3 迁移包事务：任一步失败整体回滚（user_version 不推进，库不砖化）。
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
    if current < 4 {
        conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            conn.execute_batch(MIGRATION_V4)?;
            conn.pragma_update(None, "user_version", 4)?;
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
    if current < 5 {
        // v5（Rust 迁移，需 fs::canonicalize）：合并本地意图幽灵行到扫描行。
        conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            merge_ghost_local_tracks(conn)?;
            conn.pragma_update(None, "user_version", 5)?;
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
    if current < 6 {
        // v6：歌单封面 URI（reconcile 取本地产物后回写；NULL = 尚未获取）。
        conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            conn.execute_batch(MIGRATION_V6)?;
            conn.pragma_update(None, "user_version", 6)?;
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
    Ok(())
}

/// v6：playlists 封面 URI。
const MIGRATION_V6: &str = "ALTER TABLE playlists ADD COLUMN cover_uri TEXT;";

/// v2：统一关系表（收藏/订阅 = durable outbox 一体）+ 歌单远端身份 +
/// owned 歌单曲目操作 outbox + QQ numeric song id（comment biz_id 映射）。
/// favorites 数据迁入 relations(track, liked) 后删表（媒体库内核，spec §3.1）。
const MIGRATION_V2: &str = r#"
CREATE TABLE relations (
  entity_type TEXT NOT NULL,
  provider TEXT NOT NULL,
  entity_key TEXT NOT NULL,
  relation TEXT NOT NULL,
  desired_state INTEGER NOT NULL,
  last_remote_state INTEGER,
  sync_state TEXT NOT NULL DEFAULT 'synced',
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_sync_error TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (entity_type, provider, entity_key, relation)
);
CREATE TABLE playlist_ops (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  playlist_id INTEGER NOT NULL REFERENCES playlists(id),
  op TEXT NOT NULL,
  song_key TEXT,
  song_id INTEGER,
  sync_state TEXT NOT NULL DEFAULT 'pending',
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  updated_at INTEGER
);
ALTER TABLE playlists ADD COLUMN provider TEXT NOT NULL DEFAULT 'local';
ALTER TABLE playlists ADD COLUMN remote_id TEXT;
ALTER TABLE playlists ADD COLUMN relation TEXT NOT NULL DEFAULT 'local';
ALTER TABLE playlists ADD COLUMN sync_state TEXT NOT NULL DEFAULT 'synced';
ALTER TABLE playlists ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE playlists ADD COLUMN last_sync_error TEXT;
ALTER TABLE tracks ADD COLUMN qq_song_id INTEGER;
INSERT INTO relations (entity_type, provider, entity_key, relation, desired_state, last_remote_state, sync_state, updated_at)
  SELECT 'track', t.source, t.source_key, 'liked', 1, 1, 'synced', COALESCE(f.created_at, 0)
  FROM favorites f JOIN tracks t ON t.id = f.track_id;
DROP TABLE favorites;
"#;

use rusqlite::OptionalExtension;

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;
