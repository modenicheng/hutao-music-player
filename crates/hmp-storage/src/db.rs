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
}

/// 批量元数据查询结果（投影层，`track_meta_batch`）。
#[derive(Clone, Debug)]
pub struct TrackMeta {
    pub source: String,
    pub source_key: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
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
}

/// 本地歌单内曲目。
#[derive(Clone, Debug)]
pub struct PlaylistTrackRow {
    pub position: i64,
    pub track_id: i64,
    pub title: String,
    pub source_key: String,
}

/// 媒体库（进程内单一连接；跨任务共享用 `Arc<Mutex<LibraryDb>>`，WAL 允许
/// 多进程并发读写——daemon 写入、CLI 读取）。
pub struct LibraryDb {
    conn: Connection,
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
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// 内存库（测试）。
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// 当前迁移版本（测试断言）。
    pub fn version(&self) -> rusqlite::Result<i64> {
        self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))
    }

    /// 幂等写入/更新曲目元数据；返回 track id。
    pub fn upsert_track(&mut self, t: &TrackRow) -> rusqlite::Result<i64> {
        self.conn.execute(
            r#"INSERT INTO tracks (source, source_key, title, album, artist, duration_ms, cover_uri)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT(source, source_key) DO UPDATE SET
                 title = excluded.title,
                 album = COALESCE(excluded.album, tracks.album),
                 artist = COALESCE(excluded.artist, tracks.artist),
                 duration_ms = COALESCE(excluded.duration_ms, tracks.duration_ms),
                 cover_uri = COALESCE(excluded.cover_uri, tracks.cover_uri)"#,
            params![
                t.source,
                t.source_key,
                t.title,
                t.album,
                t.artist,
                t.duration_ms,
                t.cover_uri
            ],
        )?;
        self.conn.query_row(
            "SELECT id FROM tracks WHERE source = ?1 AND source_key = ?2",
            params![t.source, t.source_key],
            |r| r.get(0),
        )
    }

    /// 记录播放会话开始（INSERT play_events）。
    pub fn record_play_start(&mut self, track_id: i64, started_at: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO play_events (track_id, started_at) VALUES (?1, ?2)",
            params![track_id, started_at],
        )?;
        Ok(())
    }

    /// 结束播放会话：闭合最近一条未结束的事件（按 track + 最新 started_at），
    /// 累加播放次数与最近播放时间。
    pub fn record_play_end(&mut self, e: &PlayEnd) -> rusqlite::Result<()> {
        let updated = self.conn.execute(
            r#"UPDATE play_events SET ended_at = ?2, listened_ms = ?3, end_reason = ?4
               WHERE id = (
                 SELECT id FROM play_events
                 WHERE track_id = ?1 AND ended_at IS NULL
                 ORDER BY started_at DESC, id DESC LIMIT 1
               )"#,
            params![e.track_id, e.ended_at, e.listened_ms, e.reason],
        )?;
        if updated > 0 {
            self.conn.execute(
                "UPDATE tracks SET play_count = play_count + 1, last_played_at = ?2 WHERE id = ?1",
                params![e.track_id, e.ended_at],
            )?;
        }
        Ok(())
    }

    /// 最近播放（默认按开始时间倒序）。
    pub fn recent_plays(&mut self, limit: u32) -> rusqlite::Result<Vec<RecentPlay>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT p.track_id, t.title, t.artist, p.started_at, p.ended_at,
                      p.listened_ms, COALESCE(p.end_reason, '')
               FROM play_events p JOIN tracks t ON t.id = p.track_id
               ORDER BY p.started_at DESC, p.id DESC LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(RecentPlay {
                track_id: r.get(0)?,
                title: r.get(1)?,
                artist: r.get(2)?,
                started_at: r.get(3)?,
                ended_at: r.get(4)?,
                listened_ms: r.get(5)?,
                reason: r.get(6)?,
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
                "SELECT source, source_key, title, artist, album FROM tracks \
                 WHERE source = ?1 AND source_key IN ({placeholders})"
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
                    .unwrap_or("未知")
                    .to_string(),
                ..Default::default()
            },
        };
        let source_key = format!("local:{}", path.display());
        let id = self.upsert_track(&TrackRow {
            source: "local",
            source_key,
            title: meta.title,
            album: meta.album,
            artist: meta.artist,
            duration_ms: meta.duration_ms,
            cover_uri: None,
        })?;
        let md = std::fs::metadata(path).ok();
        self.conn.execute(
            r#"INSERT INTO local_files (track_id, path, file_size, mtime, format, bitrate, sample_rate)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT(path) DO UPDATE SET
                 file_size = excluded.file_size,
                 mtime = excluded.mtime,
                 format = COALESCE(excluded.format, local_files.format),
                 bitrate = COALESCE(excluded.bitrate, local_files.bitrate),
                 sample_rate = COALESCE(excluded.sample_rate, local_files.sample_rate)"#,
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

    /// 收藏曲目（upsert 曲目行 + 收藏；幂等）。
    /// `source`/`source_key` 与播放历史一致（qq → mid；local → `local:<path>`）。
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
        })?;
        self.conn.execute(
            "INSERT INTO favorites (track_id, created_at) VALUES (?1, ?2)
             ON CONFLICT(track_id) DO UPDATE SET created_at = excluded.created_at",
            params![tid, now_unix()],
        )?;
        Ok(tid)
    }

    /// 取消收藏（按曲目行 id）。
    pub fn remove_favorite(&mut self, track_id: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM favorites WHERE track_id = ?1",
            params![track_id],
        )?;
        Ok(())
    }

    /// 是否已收藏（按曲目行 id）。
    pub fn is_favorite(&mut self, track_id: i64) -> rusqlite::Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM favorites WHERE track_id = ?1",
            params![track_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// 收藏列表（新→旧）。
    pub fn list_favorites(&mut self, limit: u32) -> rusqlite::Result<Vec<FavoriteRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.track_id, t.source, t.source_key, t.title, f.created_at
             FROM favorites f JOIN tracks t ON t.id = f.track_id
             ORDER BY f.created_at DESC LIMIT ?1",
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
            "SELECT p.id, p.name, p.created_at,
                    (SELECT COUNT(*) FROM playlist_tracks pt WHERE pt.playlist_id = p.id)
             FROM playlists p ORDER BY p.created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PlaylistRow {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                track_count: r.get(3)?,
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

/// 当前 unix 时间戳（秒）。
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 逐级迁移到最新 user_version。
fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current < 1 {
        conn.execute_batch(SCHEMA_V1)?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    Ok(())
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> TrackRow {
        TrackRow {
            source: "qq",
            source_key: "mid123".into(),
            title: "测试曲".into(),
            album: Some("专辑".into()),
            artist: Some("歌手".into()),
            duration_ms: Some(180_000),
            cover_uri: None,
        }
    }

    #[test]
    fn migration_creates_v1() {
        let db = LibraryDb::open_in_memory().unwrap();
        assert_eq!(db.version().unwrap(), 1);
        let mut db = db;
        assert_eq!(db.track_id("qq", "mid123").unwrap(), None);
    }

    #[test]
    fn upsert_is_idempotent() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let id1 = db.upsert_track(&row()).unwrap();
        let id2 = db.upsert_track(&row()).unwrap();
        assert_eq!(id1, id2, "UNIQUE(source, source_key) 幂等");
        // 更新标题生效
        let mut r = row();
        r.title = "新标题".into();
        db.upsert_track(&r).unwrap();
        let mut db = db;
        let id = db.track_id("qq", "mid123").unwrap().unwrap();
        let title: String = db
            .conn
            .query_row("SELECT title FROM tracks WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(title, "新标题");
    }

    #[test]
    fn play_session_roundtrip() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let id = db.upsert_track(&row()).unwrap();
        db.record_play_start(id, 1000).unwrap();
        db.record_play_end(&PlayEnd {
            track_id: id,
            ended_at: 1000 + 120,
            listened_ms: 115_000,
            reason: "ended",
        })
        .unwrap();
        let recent = db.recent_plays(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].title, "测试曲");
        assert_eq!(recent[0].listened_ms, 115_000);
        assert_eq!(recent[0].reason, "ended");
        assert_eq!(recent[0].ended_at, Some(1120));
        // play_count 累加
        let count: i64 = db
            .conn
            .query_row(
                "SELECT play_count FROM tracks WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let last: Option<i64> = db
            .conn
            .query_row(
                "SELECT last_played_at FROM tracks WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last, Some(1120));
    }

    #[test]
    fn play_end_closes_latest_open_session_only() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let id = db.upsert_track(&row()).unwrap();
        db.record_play_start(id, 1000).unwrap();
        db.record_play_start(id, 2000).unwrap(); // 换曲又回来（两段会话）
        db.record_play_end(&PlayEnd {
            track_id: id,
            ended_at: 2100,
            listened_ms: 90_000,
            reason: "ended",
        })
        .unwrap();
        let recent = db.recent_plays(10).unwrap();
        assert_eq!(recent.len(), 2);
        // 只有最新的那段被闭合
        let open: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM play_events WHERE ended_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(open, 1);
        // 播放次数只加一次（闭合了一段会话）
        let count: i64 = db
            .conn
            .query_row(
                "SELECT play_count FROM tracks WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn open_creates_dir_and_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("library.sqlite3");
        let db = LibraryDb::open(&path).unwrap();
        assert!(path.exists());
        let journal: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal, "wal");
        drop(db);
        // WAL 伴生文件存在（关闭后可清理）
        let _ = path;
    }

    #[test]
    fn batch_upsert_then_meta_batch_roundtrip() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let mut rows = vec![
            TrackRow {
                source: "qq",
                source_key: "mid-1".into(),
                title: "夜曲".into(),
                album: Some("十一月的萧邦".into()),
                artist: Some("周杰伦".into()),
                duration_ms: Some(180_000),
                cover_uri: None,
            },
            TrackRow {
                source: "qq",
                source_key: "mid-2".into(),
                title: "mid-2".into(),
                album: None,
                artist: None,
                duration_ms: None,
                cover_uri: None,
            },
            TrackRow {
                source: "local",
                source_key: "local:/m/a.flac".into(),
                title: "a.flac".into(),
                album: None,
                artist: None,
                duration_ms: None,
                cover_uri: None,
            },
        ];
        db.upsert_tracks_batch(&rows).unwrap();

        // 分 provider 批量投影；缺失 key 不返回行。
        let metas = db
            .track_meta_batch(
                "qq",
                &[
                    "mid-1".to_string(),
                    "mid-2".to_string(),
                    "mid-missing".to_string(),
                ],
            )
            .unwrap();
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].title, "夜曲");
        assert_eq!(metas[0].artist.as_deref(), Some("周杰伦"));
        let locals = db
            .track_meta_batch("local", &["local:/m/a.flac".to_string()])
            .unwrap();
        assert_eq!(locals[0].title, "a.flac");

        // 幂等重 upsert：更新标题，不重复建行。
        rows[0].title = "夜曲 2".into();
        db.upsert_tracks_batch(&rows).unwrap();
        let metas = db.track_meta_batch("qq", &["mid-1".to_string()]).unwrap();
        assert_eq!(metas[0].title, "夜曲 2");
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3, "重复批量 upsert 不建重复行");
    }

    #[test]
    fn track_meta_batch_slices_beyond_variable_limit() {
        // SQLite 变量上限 999：>999 keys 应分片查询不报错。
        let mut db = LibraryDb::open_in_memory().unwrap();
        let rows: Vec<TrackRow> = (0..1200)
            .map(|i| TrackRow {
                source: "qq",
                source_key: format!("mid-{i}"),
                title: format!("t{i}"),
                album: None,
                artist: None,
                duration_ms: None,
                cover_uri: None,
            })
            .collect();
        db.upsert_tracks_batch(&rows).unwrap();
        let keys: Vec<String> = (0..1200).map(|i| format!("mid-{i}")).collect();
        let metas = db.track_meta_batch("qq", &keys).unwrap();
        assert_eq!(metas.len(), 1200);
    }
}
