//! `db.rs` 的单元测试（模块经 `#[path]` 挂在 `db::tests`，语义不变）。

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
        ..Default::default()
    }
}

#[test]
fn migration_creates_v1() {
    let db = LibraryDb::open_in_memory().unwrap();
    assert_eq!(db.version().unwrap(), 7); // v7：非规范键本地行合并
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
        7
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
    // 播放键投影（GUI 历史页整表播放的 id 来源）
    assert_eq!(recent[0].source, "qq");
    assert_eq!(recent[0].source_key, "mid123");
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

/// 最近播放 LRU 视图：一曲一行（同曲多会话去重，行字段取最近一次会话）、
/// 再播置顶、limit 截断。会话流水视图（recent_plays）不裁剪。
#[test]
fn recent_tracks_is_lru_dedup_view() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let id_a = db.upsert_track(&row()).unwrap();
    let mut row_b = row();
    row_b.source_key = "mid456".into();
    row_b.title = "另一曲".into();
    let id_b = db.upsert_track(&row_b).unwrap();

    let play = |db: &mut LibraryDb, id: i64, started: i64, listened: i64| {
        let ev = db.record_play_start(id, started).unwrap();
        db.record_play_end(
            ev,
            &PlayEnd {
                track_id: id,
                ended_at: started + 60,
                listened_ms: listened,
                reason: "ended",
            },
        )
        .unwrap();
    };
    // A@1000 → B@2000 → A@3000：流水 3 条，LRU 应为 [A, B]。
    play(&mut db, id_a, 1_000, 10_000);
    play(&mut db, id_b, 2_000, 15_000);
    play(&mut db, id_a, 3_000, 20_000);

    assert_eq!(db.recent_plays(10).unwrap().len(), 3, "流水视图按会话记条");
    let lru = db.recent_tracks(10).unwrap();
    assert_eq!(lru.len(), 2, "LRU 视图一曲一行");
    assert_eq!(lru[0].source_key, "mid123");
    assert_eq!(lru[0].listened_ms, 20_000, "行字段取最近一次会话");
    assert_eq!(lru[1].source_key, "mid456");

    // 再播 B → 移到最前（move-to-front）。
    play(&mut db, id_b, 4_000, 5_000);
    let lru = db.recent_tracks(10).unwrap();
    assert_eq!(
        lru.iter()
            .map(|p| p.source_key.as_str())
            .collect::<Vec<_>>(),
        ["mid456", "mid123"]
    );
    // limit 截断 = 缓存容量。
    assert_eq!(db.recent_tracks(1).unwrap().len(), 1);
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
            duration_ms: Some(193_000),
            cover_uri: Some("https://y.gtimg.cn/x.jpg".into()),
            qq_song_id: None,
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
    // 扩列投影（§8.11）：时长/封面随批查询返回，队列行不再显示 0:00。
    assert_eq!(metas[0].duration_ms, Some(193_000));
    assert_eq!(
        metas[0].cover_uri.as_deref(),
        Some("https://y.gtimg.cn/x.jpg")
    );
    let locals = db
        .track_meta_batch("local", &["local:/m/a.flac".to_string()])
        .unwrap();
    assert_eq!(locals[0].title, "a.flac");
    assert_eq!(locals[0].duration_ms, None);

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
            ..Default::default()
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
        ..Default::default()
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

/// 里程碑 E：v3 迁移——local_files 文件生命周期列 + tracks 完整元数据列 +
/// track_artists/scan_roots 表（新库直达 v3）。
#[test]
fn migration_v3_adds_columns_and_tables() {
    let db = LibraryDb::open_in_memory().unwrap();
    assert_eq!(db.version().unwrap(), 7);
    let cols: Vec<String> = db
        .conn
        .prepare("PRAGMA table_info(local_files)")
        .unwrap()
        .query_map([], |r| r.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for want in [
        "mtime_ns",
        "fingerprint",
        "last_seen_generation",
        "missing",
        "scan_root_id",
    ] {
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
    for want in [
        "album_artist",
        "track_number",
        "disc_number",
        "year",
        "genre",
    ] {
        assert!(tcols.iter().any(|c| c == want), "tracks 缺列 {want}");
    }
    let n: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('track_artists','scan_roots')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2, "track_artists/scan_roots 表应存在");
}

/// v2 库在打开时原地升级到 v3：旧数据保留、新列可写。
#[test]
fn migration_v2_to_v3_upgrades_in_place() {
    let dir = std::env::temp_dir().join(format!("hmp-mig-v3-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.sqlite3");
    let _ = std::fs::remove_file(&path);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(MIGRATION_V2).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute(
            "INSERT INTO tracks (source, source_key, title, artist) VALUES ('local', 'local:/a.mp3', 'A', 'Art')",
            [],
        )
        .unwrap();
    }
    let db = LibraryDb::open(&path).unwrap();
    let n: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "迁移后旧数据保留");
    let v: i64 = db
        .conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 7);
    db.conn
        .execute("UPDATE tracks SET genre='Rock' WHERE id=1", [])
        .unwrap();
}

/// 里程碑 E：扫描生命周期——首轮新增、删除标 missing、重扫复位。
#[test]
fn scan_lifecycle_marks_missing_and_resets() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let dir = std::env::temp_dir().join(format!("hmp-scan-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.mp3");
    let b = dir.join("b.mp3");
    std::fs::write(&a, b"").unwrap();
    std::fs::write(&b, b"").unwrap();
    let b_mtime = std::fs::metadata(&b).unwrap().modified().unwrap();
    // 第一轮：全部新增。
    let (root_id, generation) = db.begin_scan(&dir).unwrap();
    assert!(matches!(
        db.record_scan_file(root_id, generation, &a, None, "fp-a")
            .unwrap(),
        ScanOutcome::Added
    ));
    assert!(matches!(
        db.record_scan_file(root_id, generation, &b, None, "fp-b")
            .unwrap(),
        ScanOutcome::Added
    ));
    assert_eq!(
        db.finish_scan(root_id, generation).unwrap(),
        0,
        "首轮无 missing"
    );
    // 同路径同指纹再扫：跳过（增量）。
    assert!(matches!(
        db.record_scan_file(root_id, generation, &a, None, "fp-a")
            .unwrap(),
        ScanOutcome::Skipped
    ));
    // 删除 b → 第二轮：b 标 missing。
    std::fs::remove_file(&b).unwrap();
    let (root_id2, generation2) = db.begin_scan(&dir).unwrap();
    assert_ne!(generation2, generation, "generation 应递增");
    db.record_scan_file(root_id2, generation2, &a, None, "fp-a")
        .unwrap();
    assert_eq!(
        db.finish_scan(root_id2, generation2).unwrap(),
        1,
        "b 应标 missing"
    );
    let miss: i64 = db
        .conn
        .query_row(
            "SELECT missing FROM local_files WHERE path LIKE '%b.mp3'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(miss, 1);
    // 重扫 b 出现 → missing 复位（外接盘场景：内容未变、mtime 不变）。
    std::fs::write(&b, b"").unwrap();
    let f = std::fs::File::options().write(true).open(&b).unwrap();
    f.set_modified(b_mtime).unwrap();
    let (root_id3, generation3) = db.begin_scan(&dir).unwrap();
    let out = db
        .record_scan_file(root_id3, generation3, &b, None, "fp-b")
        .unwrap();
    assert!(matches!(out, ScanOutcome::MissingReset));
    db.finish_scan(root_id3, generation3).unwrap();
    let miss: i64 = db
        .conn
        .query_row(
            "SELECT missing FROM local_files WHERE path LIKE '%b.mp3'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(miss, 0, "b 已复位");
}

/// 里程碑 E：移动/改名 → 指纹命中复用行（不产生孤儿曲目）。
#[test]
fn fingerprint_reuses_row_on_path_change() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let dir = std::env::temp_dir().join(format!("hmp-fp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("old.mp3");
    let new = dir.join("new.mp3");
    std::fs::write(&old, b"x").unwrap();
    let (root_id, generation) = db.begin_scan(&dir).unwrap();
    db.record_scan_file(root_id, generation, &old, None, "fp-same")
        .unwrap();
    // "移动"：旧路径没了，新路径指纹相同。
    std::fs::rename(&old, &new).unwrap();
    let (tid, _orig) = db.find_by_fingerprint("fp-same").unwrap().unwrap();
    let out = db
        .record_scan_file(root_id, generation, &new, None, "fp-same")
        .unwrap();
    assert!(matches!(out, ScanOutcome::Updated), "指纹命中复用行");
    let n: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "不产生孤儿曲目");
    let p: String = db
        .conn
        .query_row(
            "SELECT path FROM local_files WHERE track_id=?1",
            [tid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(p, new.to_str().unwrap(), "path 已更新");
}

/// 里程碑 E2：单文件删除标记（watcher Remove 事件；不删行，missing 语义与扫描一致）。
#[test]
fn mark_missing_by_path_flags_row() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let dir = std::env::temp_dir().join(format!("hmp-mm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("a.mp3");
    std::fs::write(&f, b"x").unwrap();
    let (root_id, generation) = db.begin_scan(&dir).unwrap();
    db.record_scan_file(root_id, generation, &f, None, "fp")
        .unwrap();
    let n = db.mark_missing_by_path(&f).unwrap();
    assert_eq!(n, 1);
    let miss: i64 = db
        .conn
        .query_row(
            "SELECT missing FROM local_files WHERE path=?1",
            [f.to_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(miss, 1);
    // 已删除文件的路径 → 0 行。
    assert_eq!(db.mark_missing_by_path(&dir.join("gone.mp3")).unwrap(), 0);
}

/// 里程碑 E：浏览聚合——tracks 过滤（search/artist/album/liked）、
/// albums/artists 聚合、播放源查询（album:local / artist:local）。
#[test]
fn library_browse_aggregations() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    db.conn
        .execute_batch(
            r#"
        INSERT INTO tracks (source, source_key, title, album, artist, duration_ms, year, genre) VALUES
          ('local', 'local:/a.mp3', 'A', '专辑X', '歌手1', 1000, 2020, 'Rock'),
          ('local', 'local:/b.mp3', 'B', '专辑X', '歌手1', 2000, 2020, 'Rock');
        INSERT INTO local_files (track_id, path, last_seen_generation, missing, scan_root_id) VALUES
          (1, '/a.mp3', 1, 0, 1),
          (2, '/b.mp3', 1, 0, 1);
        INSERT INTO track_artists (track_id, artist, position) VALUES
          (1, '歌手1', 0), (1, '歌手2', 1), (2, '歌手1', 0);
    "#,
        )
        .unwrap();
    db.set_relation("track", "local", "local:/b.mp3", "liked", true)
        .unwrap();
    // 全量
    let all = db.library_tracks(None, None, None, false).unwrap();
    assert_eq!(all.len(), 2);
    // search
    let s = db.library_tracks(Some("B"), None, None, false).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].title, "B");
    // artist（track_artists 多值命中）
    let ar = db.library_tracks(None, Some("歌手2"), None, false).unwrap();
    assert_eq!(ar.len(), 1);
    assert_eq!(ar[0].title, "A");
    // album
    let al = db.library_tracks(None, None, Some("专辑X"), false).unwrap();
    assert_eq!(al.len(), 2);
    // liked
    let lk = db.library_tracks(None, None, None, true).unwrap();
    assert_eq!(lk.len(), 1);
    assert_eq!(lk[0].source_key, "local:/b.mp3");
    // albums 聚合
    let albums = db.library_albums(None).unwrap();
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].album, "专辑X");
    assert_eq!(albums[0].track_count, 2);
    assert_eq!(albums[0].year, Some(2020));
    // artists 聚合（多值拆行）
    let artists = db.library_artists().unwrap();
    assert_eq!(artists.len(), 2);
    assert!(
        artists
            .iter()
            .any(|a| a.artist == "歌手1" && a.track_count == 2)
    );
    assert!(
        artists
            .iter()
            .any(|a| a.artist == "歌手2" && a.track_count == 1)
    );
    // 播放源查询
    let by_album = db.local_tracks_by_album("专辑X").unwrap();
    assert_eq!(by_album.len(), 2);
    let by_artist = db.local_tracks_by_artist("歌手2").unwrap();
    assert_eq!(by_artist.len(), 1);
    // missing 可见
    db.conn
        .execute("UPDATE local_files SET missing=1 WHERE track_id=2", [])
        .unwrap();
    let rows = db.library_tracks(None, None, None, false).unwrap();
    assert!(rows.iter().find(|r| r.track_id == 2).unwrap().missing);
    // scan_roots 查询
    assert!(db.scan_roots().unwrap().is_empty());
}

#[test]
fn scan_root_for_picks_longest_prefix() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let dir = std::env::temp_dir().join(format!("hmp-sr-{}", std::process::id()));
    let inner = dir.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    let (rid1, gen1) = db.begin_scan(&dir).unwrap();
    let (rid2, gen2) = db.begin_scan(&inner).unwrap();
    // 嵌套 root：/music/inner/x.mp3 应命中内层 root。
    let f = inner.join("x.mp3");
    std::fs::write(&f, b"x").unwrap();
    let hit = db.scan_root_for(&f).unwrap().unwrap();
    assert_eq!(hit, (rid2, gen2), "嵌套 root 取最长前缀");
    // 外层文件命中外层 root。
    let outer = dir.join("y.mp3");
    std::fs::write(&outer, b"y").unwrap();
    let hit2 = db.scan_root_for(&outer).unwrap().unwrap();
    assert_eq!(hit2, (rid1, gen1));
}

#[test]
fn local_playlist_stubs_lists_ordered_tracks() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let pid = db.create_playlist("本地歌单").unwrap();
    // 混排：QQ 曲目 + 本地曲目。
    db.add_playlist_track(pid, "qq", "mid-1", "QQ 歌").unwrap();
    db.add_playlist_track(pid, "local", "local:/a.mp3", "本地歌")
        .unwrap();
    let rows = db.local_playlist_stubs(pid).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].source_key, "mid-1");
    assert_eq!(rows[0].title, "QQ 歌");
    assert_eq!(rows[1].source_key, "local:/a.mp3");
    // 不存在 → 空列表。
    assert!(db.local_playlist_stubs(999).unwrap().is_empty());
}

/// 回归（M8 桌面接线发现的幽灵行 bug）：收藏/歌单写路径给非 canonical
/// `local:<path>`（symlink/旧挂载点）时，必须归一到与扫描器同一键，
/// 否则同一文件在 tracks 里两行、列表富化失联。
#[test]
fn local_intent_writes_normalize_to_canonical_key() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.wav");
    std::fs::write(&real, b"x").unwrap();
    let alias_dir = dir.path().join("alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path(), &alias_dir).unwrap();
    #[cfg(not(unix))]
    let _ = alias_dir;
    #[cfg(unix)]
    {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let meta = crate::local::LocalMeta {
            title: "真实标题".into(),
            ..Default::default()
        };
        let tid = db.add_local_file(&real, Some(&meta)).unwrap();
        let alias_key = format!("local:{}", alias_dir.join("real.wav").display());
        assert_ne!(alias_key, format!("local:{}", real.display()));

        db.add_favorite("local", &alias_key, &alias_key).unwrap();
        // 单行：意图 upsert 落在扫描行上，不产生幽灵
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM tracks WHERE source = 'local'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "意图写必须与扫描行合并");
        // relation 记在规范键上，且标题未被键名踩掉
        assert_eq!(
            db.relation_desired("track", "local", &alias_key, "liked")
                .unwrap(),
            Some(true)
        );
        let title: String = db
            .conn
            .query_row(
                "SELECT title FROM tracks WHERE id = ?1",
                params![tid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "真实标题");
    }
}

/// 意图方以键代标题时不得踩掉已有元数据（COALESCE 语义的标题特例）。
#[test]
fn upsert_intent_title_does_not_stomp_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.wav");
    std::fs::write(&path, b"x").unwrap();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let meta = crate::local::LocalMeta {
        title: "真标题".into(),
        ..Default::default()
    };
    db.add_local_file(&path, Some(&meta)).unwrap();
    let key = format!("local:{}", path.display());
    db.upsert_track(&TrackRow {
        source: "local",
        source_key: key.clone(),
        title: key.clone(),
        ..Default::default()
    })
    .unwrap();
    // upsert 侧 canonical_local_key 归一（Windows TEMP 环境变量拼写与盘上
    // 真实大小写可能不同），查询按归一化后的键取行。
    let canon_key = format!("local:{}", crate::canonical_display_path(&path).display());
    let title: String = db
        .conn
        .query_row(
            "SELECT title FROM tracks WHERE source='local' AND source_key=?1",
            params![canon_key],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(title, "真标题");
}

/// v5 迁移：既有库里的幽灵行（非 canonical 键 + 无 local_files）合并进
/// 扫描行，relations/歌单链接跟随重指向，幽灵行删除。幂等。
#[test]
fn merge_ghost_local_tracks_remaps_relations_and_playlists() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("song.wav");
    std::fs::write(&real, b"x").unwrap();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let meta = crate::local::LocalMeta {
        title: "真标题".into(),
        ..Default::default()
    };
    let tid = db.add_local_file(&real, Some(&meta)).unwrap();
    let canon_key = format!("local:{}", real.display());

    // 直接造 pre-fix 形态的幽灵（API 现已归一化，只能 raw SQL 造）：
    // Unix = symlink 别名拼写；Windows = verbatim \?\ 前缀拼写。
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path(), dir.path().join("alias")).unwrap();
    #[cfg(unix)]
    let ghost_key = format!(
        "local:{}",
        dir.path().join("alias").join("song.wav").display()
    );
    #[cfg(windows)]
    let ghost_key = format!("local:{}", std::fs::canonicalize(&real).unwrap().display());
    db.conn
        .execute(
            "INSERT INTO tracks (source, source_key, title) VALUES ('local', ?1, ?1)",
            params![ghost_key],
        )
        .unwrap();
    let gid: i64 = db
        .conn
        .query_row(
            "SELECT id FROM tracks WHERE source='local' AND source_key=?1",
            params![ghost_key],
            |r| r.get(0),
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO relations (entity_type, provider, entity_key, relation, \
             desired_state, sync_state, updated_at) \
             VALUES ('track','local',?1,'liked',1,'synced',0)",
            params![ghost_key],
        )
        .unwrap();
    db.create_playlist("深夜循环").unwrap();
    db.conn
        .execute(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at) \
             VALUES (1, ?1, 0, 0)",
            params![gid],
        )
        .unwrap();

    db.merge_ghost_local_tracks().unwrap();

    // 幽灵删除、行数回到 1；relation 与歌单链接都指向扫描行
    let count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE source = 'local'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        db.relation_desired("track", "local", &canon_key, "liked")
            .unwrap(),
        Some(true)
    );
    let linked: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM playlist_tracks WHERE track_id = ?1",
            params![tid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(linked, 1);
    // 幂等：再跑一遍零变化
    db.merge_ghost_local_tracks().unwrap();
    assert_eq!(
        db.conn
            .query_row(
                "SELECT COUNT(*) FROM tracks WHERE source='local'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

/// v7 迁移：带 local_files 子行的非规范键扫描行（v5 幽灵谓词不命中的变体，
/// §18 实锤的 verbatim 重复行）合并进规范键扫描行，relations/歌单链接跟随
/// 重指向，verbatim 行连同 local_files 删除。幂等。
#[test]
fn merge_noncanonical_local_tracks_merges_scan_rows() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("song.flac");
    std::fs::write(&real, b"x").unwrap();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let meta = crate::local::LocalMeta {
        title: "真标题".into(),
        ..Default::default()
    };
    let tid = db.add_local_file(&real, Some(&meta)).unwrap();
    let canon_key = format!("local:{}", real.display());

    // 直接造 pre-fix 形态的 verbatim 扫描行（API 现已归一化，只能 raw SQL 造）：
    // tracks.source_key 与 local_files.path 均带非规范拼写。
    // Windows = canonicalize 的 \\?\ 前缀形态；Unix = 同目录 /./ 重复拼写。
    #[cfg(windows)]
    let raw = std::fs::canonicalize(&real).unwrap();
    #[cfg(unix)]
    let raw = dir.path().join(".").join("song.flac");
    let ghost_key = format!("local:{}", raw.display());
    assert_ne!(ghost_key, canon_key, "测试前置：ghost 键必须非规范");
    db.conn
        .execute(
            "INSERT INTO tracks (source, source_key, title) VALUES ('local', ?1, ?1)",
            params![ghost_key],
        )
        .unwrap();
    let gid: i64 = db
        .conn
        .query_row(
            "SELECT id FROM tracks WHERE source='local' AND source_key=?1",
            params![ghost_key],
            |r| r.get(0),
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO local_files (track_id, path) VALUES (?1, ?2)",
            params![gid, raw.display().to_string()],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO relations (entity_type, provider, entity_key, relation, \
             desired_state, sync_state, updated_at) \
             VALUES ('track','local',?1,'liked',1,'synced',0)",
            params![ghost_key],
        )
        .unwrap();
    db.create_playlist("深夜循环").unwrap();
    db.conn
        .execute(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at) \
             VALUES (1, ?1, 0, 0)",
            params![gid],
        )
        .unwrap();
    // 播放历史挂在 verbatim 行上（resolve_local 播放即建行；play_events FK 无
    // CASCADE，不先并入会让 DELETE tracks 失败、迁移整体回滚——§18 实机首跑
    // 即中招）。
    db.conn
        .execute(
            "INSERT INTO play_events (track_id, started_at, listened_ms) VALUES (?1, 1, 60)",
            params![gid],
        )
        .unwrap();

    db.merge_noncanonical_local_tracks().unwrap();

    // verbatim 行删除、行数回到 1；relation 与歌单链接都指向扫描行
    let count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE source = 'local'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        db.relation_desired("track", "local", &canon_key, "liked")
            .unwrap(),
        Some(true)
    );
    let linked: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM playlist_tracks WHERE track_id = ?1",
            params![tid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(linked, 1);
    let history: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM play_events WHERE track_id = ?1",
            params![tid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(history, 1, "播放历史应并入存活行而非阻塞删除");
    let orphaned: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM local_files WHERE track_id = ?1",
            params![gid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphaned, 0, "verbatim 行的 local_files 子行应一并删除");
    // 幂等：再跑一遍零变化
    db.merge_noncanonical_local_tracks().unwrap();
    assert_eq!(
        db.conn
            .query_row(
                "SELECT COUNT(*) FROM tracks WHERE source='local'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

/// v7 迁移 re-key 分支：规范行缺席 → tracks.source_key / local_files.path
/// 就地归一；归一后 add_local_file 幂等命中同一行，不再衍生重复行。
#[test]
fn merge_noncanonical_local_tracks_rekeys_when_target_absent() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("lonely.flac");
    std::fs::write(&real, b"x").unwrap();
    let mut db = LibraryDb::open_in_memory().unwrap();
    #[cfg(windows)]
    let raw = std::fs::canonicalize(&real).unwrap();
    #[cfg(unix)]
    let raw = dir.path().join(".").join("lonely.flac");
    let ghost_key = format!("local:{}", raw.display());
    db.conn
        .execute(
            "INSERT INTO tracks (source, source_key, title) VALUES ('local', ?1, ?1)",
            params![ghost_key],
        )
        .unwrap();
    let gid: i64 = db
        .conn
        .query_row(
            "SELECT id FROM tracks WHERE source='local' AND source_key=?1",
            params![ghost_key],
            |r| r.get(0),
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO local_files (track_id, path) VALUES (?1, ?2)",
            params![gid, raw.display().to_string()],
        )
        .unwrap();

    db.merge_noncanonical_local_tracks().unwrap();

    let canon_key = format!("local:{}", crate::canonical_display_path(&real).display());
    let key: String = db
        .conn
        .query_row(
            "SELECT source_key FROM tracks WHERE source='local'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(key, canon_key);
    let path: String = db
        .conn
        .query_row(
            "SELECT path FROM local_files WHERE track_id = ?1",
            params![gid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(path, canon_key.strip_prefix("local:").unwrap());
    // 归一后 add_local_file 幂等命中同一行（不衍生重复行）
    let tid = db.add_local_file(&real, None).unwrap();
    assert_eq!(tid, gid);
    let count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE source='local'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

/// CoverGet 本地产物回写：按远程 URL 匹配改写 cover_uri（同 URL 多曲全改；
/// 已是目标值时幂等零写）。
#[test]
fn rebind_cover_url_rewrites_matching_tracks() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let row = |key: &str, url: &str| crate::TrackRow {
        source: "qq",
        source_key: key.into(),
        title: "晴天".into(),
        cover_uri: Some(url.into()),
        ..Default::default()
    };
    let url_x = "https://y.gtimg.cn/music/photo_new/T002R300x300M000X.jpg";
    let url_y = "https://y.gtimg.cn/music/photo_new/T002R300x300M000Y.jpg";
    let a = db.upsert_track(&row("a", url_x)).unwrap();
    let b = db.upsert_track(&row("b", url_x)).unwrap();
    let c = db.upsert_track(&row("c", url_y)).unwrap();

    let local = format!("file://{}", "C:/Users/u/AppData/Local/hmp/covers/aa.jpg");
    let n = db.rebind_cover_url(url_x, &local).unwrap();
    assert_eq!(n, 2, "同 URL 两行全改");
    for tid in [a, b] {
        let uri: String = db
            .conn
            .query_row(
                "SELECT cover_uri FROM tracks WHERE id = ?1",
                params![tid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(uri, local, "回写为本地产物 URI");
    }
    let other: String = db
        .conn
        .query_row(
            "SELECT cover_uri FROM tracks WHERE id = ?1",
            params![c],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(other, url_y, "其他 URL 不受影响");

    // 幂等：已是目标值 → 零行写入
    let n2 = db.rebind_cover_url(url_x, &local).unwrap();
    assert_eq!(n2, 0, "重复回写幂等");
}
