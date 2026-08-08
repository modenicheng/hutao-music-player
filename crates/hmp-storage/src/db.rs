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
}
