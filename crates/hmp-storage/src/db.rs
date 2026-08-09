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
            qq_song_id: None,
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
             p.sync_state, p.retry_count, p.last_sync_error, p.updated_at,\n                    (SELECT COUNT(*) FROM playlist_tracks pt WHERE pt.playlist_id = p.id) \
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
                track_count: r.get(10)?,
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
             p.sync_state, p.retry_count, p.last_sync_error, p.updated_at,\n                    (SELECT COUNT(*) FROM playlist_tracks pt WHERE pt.playlist_id = p.id) \
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
                track_count: r.get(10)?,
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
            qq_song_id: None,
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
    Ok(())
}

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
            qq_song_id: None,
        }
    }

    #[test]
    fn migration_creates_v1() {
        let db = LibraryDb::open_in_memory().unwrap();
        assert_eq!(db.version().unwrap(), 2); // v2：relations/outbox 迁移
        let mut db = db;
        assert_eq!(db.track_id("qq", "mid123").unwrap(), None);
    }

    /// 双向 reconcile：远端缺席 → desired=0（synced 行）；pending 行不受影响。
    #[test]
    fn reconcile_remove_absent_respects_pending() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        // 两条 synced 收藏：一条仍在远端，一条缺席。
        db.add_favorite("qq", "keep", "keep").unwrap();
        db.mark_relation_synced("track", "qq", "keep", "liked")
            .unwrap();
        db.add_favorite("qq", "gone", "gone").unwrap();
        db.mark_relation_synced("track", "qq", "gone", "liked")
            .unwrap();
        // 一条 pending（本地意图）——远端缺席也不得覆盖。
        db.add_favorite("qq", "pending-one", "pending-one").unwrap();
        db.reconcile_remove_absent("track", "qq", "liked", &["keep".to_string()])
            .unwrap();
        assert_eq!(
            db.relation_desired("track", "qq", "keep", "liked").unwrap(),
            Some(true),
            "仍在远端：保留"
        );
        assert_eq!(
            db.relation_desired("track", "qq", "gone", "liked").unwrap(),
            Some(false),
            "远端缺席：desired=0"
        );
        assert_eq!(
            db.relation_desired("track", "qq", "pending-one", "liked")
                .unwrap(),
            Some(true),
            "pending 本地意图：远端缺席不覆盖"
        );
        assert_eq!(
            db.relations_pending().unwrap().len(),
            1,
            "仅 pending 行留在 outbox"
        );
    }

    /// provider 过滤：local 收藏（synced）不受 QQ 快照缺席清理影响。
    #[test]
    fn reconcile_remove_absent_keeps_local_provider() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.add_favorite("local", "local:/m/a.flac", "a.flac")
            .unwrap();
        db.mark_relation_synced("track", "local", "local:/m/a.flac", "liked")
            .unwrap();
        db.add_favorite("qq", "gone", "gone").unwrap();
        db.mark_relation_synced("track", "qq", "gone", "liked")
            .unwrap();
        // QQ 快照为空 → 只清 QQ 的 synced 行；local 收藏保留。
        db.reconcile_remove_absent("track", "qq", "liked", &[])
            .unwrap();
        assert_eq!(
            db.relation_desired("track", "local", "local:/m/a.flac", "liked")
                .unwrap(),
            Some(true),
            "local 收藏不受 QQ 快照影响"
        );
        assert_eq!(
            db.relation_desired("track", "qq", "gone", "liked").unwrap(),
            Some(false),
            "QQ synced 行被全清"
        );
    }

    /// 空 present：远端快照为 0 条 → 全量清理该 provider 的 synced 行（pending 保留）。
    #[test]
    fn reconcile_remove_absent_empty_present_clears_all_synced() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.add_favorite("qq", "a", "a").unwrap();
        db.mark_relation_synced("track", "qq", "a", "liked")
            .unwrap();
        db.add_favorite("qq", "b", "b").unwrap();
        db.mark_relation_synced("track", "qq", "b", "liked")
            .unwrap();
        db.add_favorite("qq", "c-pending", "c-pending").unwrap(); // pending 保留
        db.reconcile_remove_absent("track", "qq", "liked", &[])
            .unwrap();
        assert_eq!(
            db.relation_desired("track", "qq", "a", "liked").unwrap(),
            Some(false)
        );
        assert_eq!(
            db.relation_desired("track", "qq", "b", "liked").unwrap(),
            Some(false)
        );
        assert_eq!(
            db.relation_desired("track", "qq", "c-pending", "liked")
                .unwrap(),
            Some(true),
            "pending 保留"
        );
    }

    /// 迁移回滚：v2 中途失败 → 整体回滚（user_version 不推进、favorites 表仍在）。
    #[test]
    fn migration_v2_rolls_back_on_failure() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        // 预建与 MIGRATION_V2 冲突的表：v2 的 CREATE TABLE relations 会失败。
        conn.execute_batch("CREATE TABLE relations (id INTEGER PRIMARY KEY);")
            .unwrap();
        let result = super::migrate(&mut conn);
        assert!(result.is_err(), "v2 迁移应失败");
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 1, "回滚后 user_version 不推进");
        // favorites 表仍在（v2 的 DROP 未执行）——用 exists 检查。
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='favorites'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "favorites 表未被 DROP");
    }

    /// 手工构造 v1 库（含 favorites 数据）→ 跑 migrate → favorites 迁入 relations。
    #[test]
    fn migration_v2_migrates_favorites_into_relations() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO tracks (source, source_key, title) VALUES ('qq', 'mid-1', '夜曲')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (source, source_key, title) VALUES ('local', 'local:/m/a.flac', 'a')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO favorites (track_id, created_at) VALUES (1, 100)",
            [],
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        // favorites 表已删除；数据在 relations（track/liked，synced）。
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM relations WHERE entity_type='track' AND relation='liked'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "v1 收藏应迁入 relations");
        let fav_err = conn.execute("SELECT * FROM favorites", []);
        assert!(fav_err.is_err(), "favorites 表应被 DROP");
        // qq 行 desired=1/last_remote=1/synced。
        let (desired, remote, sync): (i64, i64, String) = conn
            .query_row(
                "SELECT desired_state, last_remote_state, sync_state FROM relations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((desired, remote, sync.as_str()), (1, 1, "synced"));
    }

    /// set_relation 的 settled 优化：已同步且意图与远端一致 → 不进 outbox。
    #[test]
    fn set_relation_settled_skips_outbox() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.add_favorite("qq", "mid-s", "mid-s").unwrap(); // pending
        // 模拟 SyncWorker 同步成功（desired=1 已同步）。
        db.mark_relation_synced("track", "qq", "mid-s", "liked")
            .unwrap();
        assert_eq!(db.relations_pending().unwrap().len(), 0);
        // 再次收藏（意图与远端一致）→ settled 路径，不置 pending。
        db.add_favorite("qq", "mid-s", "mid-s").unwrap();
        assert_eq!(
            db.relations_pending().unwrap().len(),
            0,
            "settled 一致时不得重新进 outbox"
        );
        // 取消收藏（意图变化）→ 进 outbox。
        db.remove_favorite(1).unwrap();
        assert_eq!(db.relations_pending().unwrap().len(), 1);
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
        let event_id = db.record_play_start(id, 1000).unwrap();
        db.record_play_end(
            event_id,
            &PlayEnd {
                track_id: id,
                ended_at: 1000 + 120,
                listened_ms: 115_000,
                reason: "ended",
            },
        )
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
        let id2 = db.record_play_start(id, 2000).unwrap(); // 换曲又回来（两段会话）
        db.record_play_end(
            id2,
            &PlayEnd {
                track_id: id,
                ended_at: 2100,
                listened_ms: 90_000,
                reason: "ended",
            },
        )
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

    /// 无 relations 行时 is_favorite 返回 Ok(false) 而非 QueryReturnedNoRows
    /// （relation_desired 无行 → None，修复前 query_row 直接报错）。
    #[test]
    fn is_favorite_without_relation_row_is_false() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let id = db.upsert_track(&row()).unwrap();
        assert!(!db.is_favorite(id).unwrap(), "无 relations 行应视为未收藏");
        // 对照：收藏后为 true，取消后回到 false。
        db.add_favorite("qq", "mid123", "mid123").unwrap();
        assert!(db.is_favorite(id).unwrap());
        db.remove_favorite(id).unwrap();
        assert!(!db.is_favorite(id).unwrap());
    }

    /// 每次 record_play_start 返回独立事件 id；record_play_end 按 id 精确闭合
    /// （同曲多段会话互不影响）；重复闭合同 id 幂等（不重复累加播放次数）。
    #[test]
    fn play_start_returns_id_and_end_closes_by_id() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let id = db.upsert_track(&row()).unwrap();
        let id1 = db.record_play_start(id, 1000).unwrap();
        let id2 = db.record_play_start(id, 2000).unwrap();
        assert_ne!(id1, id2, "每次开始都返回独立事件 id");
        // 按 id1 精确闭合：只影响第一条。
        db.record_play_end(
            id1,
            &PlayEnd {
                track_id: id,
                ended_at: 3000,
                listened_ms: 500,
                reason: "ended",
            },
        )
        .unwrap();
        let open: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM play_events WHERE ended_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(open, 1, "只有第二条仍 open");
        // 重复闭合同 id：幂等（updated=0，play_count 不重复累加）。
        db.record_play_end(
            id1,
            &PlayEnd {
                track_id: id,
                ended_at: 3000,
                listened_ms: 500,
                reason: "ended",
            },
        )
        .unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT play_count FROM tracks WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "重复闭合不重复累加");
    }

    /// 启动恢复：遗留 open session 全部闭合（end_reason='interrupted'），幂等。
    #[test]
    fn close_stale_sessions_closes_open_events_idempotently() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let id = db.upsert_track(&row()).unwrap();
        db.record_play_start(id, 1000).unwrap();
        db.record_play_start(id, 2000).unwrap();
        assert_eq!(
            db.close_stale_sessions().unwrap(),
            2,
            "两条 open session 被闭合"
        );
        // 全部闭合且 reason 为 interrupted。
        let open: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM play_events WHERE ended_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(open, 0);
        let reason: String = db
            .conn
            .query_row(
                "SELECT end_reason FROM play_events WHERE track_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, "interrupted");
        assert_eq!(
            db.close_stale_sessions().unwrap(),
            0,
            "重复调用幂等（无遗留 open session）"
        );
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
                qq_song_id: None,
            },
            TrackRow {
                source: "qq",
                source_key: "mid-2".into(),
                title: "mid-2".into(),
                album: None,
                artist: None,
                duration_ms: None,
                cover_uri: None,
                qq_song_id: None,
            },
            TrackRow {
                source: "local",
                source_key: "local:/m/a.flac".into(),
                title: "a.flac".into(),
                album: None,
                artist: None,
                duration_ms: None,
                cover_uri: None,
                qq_song_id: None,
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
                qq_song_id: None,
            })
            .collect();
        db.upsert_tracks_batch(&rows).unwrap();
        let keys: Vec<String> = (0..1200).map(|i| format!("mid-{i}")).collect();
        let metas = db.track_meta_batch("qq", &keys).unwrap();
        assert_eq!(metas.len(), 1200);
    }

    /// 组合方法单事务：op 入队失败 → 本地关联整体回滚（无"本地已改、远端意图丢失"窗口）。
    #[test]
    fn add_owned_track_with_op_rolls_back_on_op_failure() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let pid = db.create_playlist("p").unwrap();
        db.upsert_track(&TrackRow {
            source: "qq",
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
        assert_eq!(
            t, 1,
            "预插的 tracks 行仍在（组合方法未产生额外行）；回滚不误删既有数据"
        );
    }

    #[test]
    fn add_owned_track_with_op_commits_atomically() {
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
            .query_row("SELECT sync_state FROM playlists WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(st, "synced", "pending 标记不得残留（整体回滚）");
    }
}
