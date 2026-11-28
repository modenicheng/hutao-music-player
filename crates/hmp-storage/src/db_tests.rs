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
    assert_eq!(db.version().unwrap(), 9); // v9：曲目 mid + 本地标签版本
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

/// AUDIT §30.1（P0）回归：在场键**整体**排除。旧实现按 500 分片、每片执行
/// `NOT IN (本片)`——片与片互视为缺席，快照超过一片时该 provider 的 synced
/// 行被全清（866 首「我喜欢」每轮同步后清零）。
fn assert_remove_absent_keeps_present(present: usize) {
    let mut db = LibraryDb::open_in_memory().unwrap();
    // present 条在场（全部 synced）+ 1 条缺席 + 1 条 pending（本地意图）。
    let keys: Vec<String> = (0..present).map(|i| format!("mid-{i}")).collect();
    for k in &keys {
        db.add_favorite("qq", k, k).unwrap();
        db.mark_relation_synced("track", "qq", k, "liked").unwrap();
    }
    db.add_favorite("qq", "gone", "gone").unwrap();
    db.mark_relation_synced("track", "qq", "gone", "liked")
        .unwrap();
    db.add_favorite("qq", "pending-one", "pending-one").unwrap();
    db.reconcile_remove_absent("track", "qq", "liked", &keys)
        .unwrap();

    let (kept, cleared): (i64, i64) = db
        .conn
        .query_row(
            "SELECT COUNT(*) FILTER (WHERE desired_state=1 AND sync_state='synced'), \
             COUNT(*) FILTER (WHERE desired_state=0) FROM relations \
             WHERE entity_type='track' AND provider='qq' AND relation='liked'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kept, present as i64, "在场行全部保持 synced + desired=1");
    assert_eq!(cleared, 1, "仅缺席的那一行被置 0");
    // 抽查旧分片边界两侧：片首、首片末行、次片首行、末行。
    for k in [&keys[0], &keys[499], &keys[500], &keys[present - 1]] {
        assert_eq!(
            db.relation_desired("track", "qq", k, "liked").unwrap(),
            Some(true),
            "在场 {k} 保留"
        );
    }
    assert_eq!(
        db.relation_desired("track", "qq", "gone", "liked").unwrap(),
        Some(false),
        "远端缺席：desired=0"
    );
    // pending 保护不回归：本地意图不被远端缺席覆盖，仍留在 outbox。
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
    // 用户可见视图：收藏列表原样保留（present 条在场 + 1 条 pending；gone 不再出现）。
    let favs = db.list_favorites(present as u32 + 10).unwrap();
    assert_eq!(favs.len(), present + 1, "list_favorites 不丢在场曲");
    assert!(favs.iter().all(|f| f.source_key != "gone"));
}

#[test]
fn reconcile_remove_absent_600_present_keeps_all() {
    assert_remove_absent_keeps_present(600);
}

#[test]
fn reconcile_remove_absent_1200_present_keeps_all() {
    assert_remove_absent_keeps_present(1200);
}

/// AUDIT §30.1：`delete_playlists_absent` 同款——600 个在场 + 1 个缺席的
/// subscribed 歌单，只删缺席的。
#[test]
fn delete_playlists_absent_600_present_deletes_only_absent() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let present: Vec<String> = (0..600).map(|i| format!("disstid-{i}")).collect();
    for id in &present {
        db.reconcile_playlist(id, "歌单", "subscribed").unwrap();
    }
    db.reconcile_playlist("gone", "缺席歌单", "subscribed")
        .unwrap();
    // relation 谓词之外（本地歌单）不受影响。
    db.create_playlist("本地歌单").unwrap();
    db.delete_playlists_absent("subscribed", &present).unwrap();

    let rows = db.list_playlists().unwrap();
    assert_eq!(rows.len(), 601, "600 在场 + 1 本地歌单");
    assert!(
        rows.iter().all(|p| p.remote_id.as_deref() != Some("gone")),
        "只删缺席歌单"
    );
    for id in ["disstid-0", "disstid-499", "disstid-500", "disstid-599"] {
        assert!(
            rows.iter().any(|p| p.remote_id.as_deref() == Some(id)),
            "在场 {id} 保留"
        );
    }
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
        9
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

/// AUDIT §30.7：批量 upsert 的标题保护须与单条 `upsert_track` 同规则——
/// 以（原始）键代标题的 stub 行不得踩掉已有真实标题（否则解析好的标题被打回
/// mid）；真实标题照常更新。
#[test]
fn batch_upsert_intent_title_does_not_stomp_metadata() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    db.upsert_track(&TrackRow {
        source: "qq",
        source_key: "mid-1".into(),
        title: "夜曲".into(),
        artist: Some("周杰伦".into()),
        ..Default::default()
    })
    .unwrap();
    let stub = |key: &str, title: &str| TrackRow {
        source: "qq",
        source_key: key.into(),
        title: title.into(),
        ..Default::default()
    };

    // 标题 == key 的 stub 行：真实标题保持不变。
    db.upsert_tracks_batch(&[stub("mid-1", "mid-1")]).unwrap();
    let metas = db.track_meta_batch("qq", &["mid-1".to_string()]).unwrap();
    assert_eq!(metas[0].title, "夜曲", "stub 标题不得踩掉真实标题");

    // 另一个真实标题：照常更新。
    db.upsert_tracks_batch(&[stub("mid-1", "夜曲（Live）")])
        .unwrap();
    let metas = db.track_meta_batch("qq", &["mid-1".to_string()]).unwrap();
    assert_eq!(metas[0].title, "夜曲（Live）");

    // 全新行：标题 == key 照常落库（保护只作用于冲突更新）。
    db.upsert_tracks_batch(&[stub("mid-2", "mid-2")]).unwrap();
    let metas = db.track_meta_batch("qq", &["mid-2".to_string()]).unwrap();
    assert_eq!(metas[0].title, "mid-2");
}

#[test]
fn track_meta_batch_slices_beyond_variable_limit() {
    // SQL 变量上限（内置 SQLite 默认 32766，并非 999）→ 按 500 分片只为稳妥：
    // >500 keys 分片查询不报错。
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
    assert_eq!(db.version().unwrap(), 9);
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
    assert_eq!(v, 9);
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

/// AUDIT §30.2.4：修复前移动/改名（指纹命中复用行）只改 `local_files.path` 与
/// `tracks.source_key`，relations 仍挂在旧键——而 list_favorites /
/// liked_only / is_favorite 都按 `tracks.source_key = relations.entity_key`
/// 关联，「喜欢」静默失联。关系行须随身份搬到新键（sync_state 原样保留）。
#[test]
fn scan_move_rekeys_local_relations() {
    let dir = tempfile::tempdir().unwrap();
    // 扫描器传 canonical 路径：与 set_relation 的键归一同一拼写。
    let a = crate::canonical_display_path(&dir.path().join("a.mp3"));
    let b = crate::canonical_display_path(&dir.path().join("b.mp3"));
    std::fs::write(&a, b"x").unwrap();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let (root_id, generation) = db.begin_scan(dir.path()).unwrap();
    assert!(matches!(
        db.record_scan_file(root_id, generation, &a, None, "fp-same")
            .unwrap(),
        ScanOutcome::Added
    ));
    let key_a = format!("local:{}", a.display());
    let key_b = format!("local:{}", b.display());
    db.set_relation("track", "local", &key_a, "liked", true)
        .unwrap();
    db.mark_relation_synced("track", "local", &key_a, "liked")
        .unwrap();

    // 「移动」：同指纹 + 同 mtime_ns（rename 不改 mtime）。
    std::fs::rename(&a, &b).unwrap();
    let out = db
        .record_scan_file(root_id, generation, &b, None, "fp-same")
        .unwrap();
    assert!(matches!(out, ScanOutcome::Updated), "指纹命中复用行");

    let favs = db.list_favorites(10).unwrap();
    assert_eq!(favs.len(), 1, "移动后「喜欢」仍在收藏列表");
    assert_eq!(favs[0].source_key, key_b);
    assert_eq!(
        db.relation_desired("track", "local", &key_b, "liked")
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        db.relation_desired("track", "local", &key_a, "liked")
            .unwrap(),
        None,
        "旧键下没有残留行"
    );
    // 同步状态原样保留：搬迁不把已 synced 的意图重新推进 outbox。
    assert!(db.relations_pending().unwrap().is_empty());
    // 其余视图同口径。
    let tid = db.track_id("local", &key_b).unwrap().unwrap();
    assert!(db.is_favorite(tid).unwrap());
    let liked = db.library_tracks(None, None, None, true).unwrap();
    assert_eq!(liked.len(), 1);
    assert_eq!(liked[0].source_key, key_b);
}

/// 同上，新键已有同一 relation 的行：弃旧保既有（不撞主键、不丢既有意图）。
#[test]
fn scan_move_drops_old_relation_when_new_key_taken() {
    let dir = tempfile::tempdir().unwrap();
    let a = crate::canonical_display_path(&dir.path().join("a.mp3"));
    let b = crate::canonical_display_path(&dir.path().join("b.mp3"));
    std::fs::write(&a, b"x").unwrap();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let (root_id, generation) = db.begin_scan(dir.path()).unwrap();
    db.record_scan_file(root_id, generation, &a, None, "fp-same")
        .unwrap();
    let key_a = format!("local:{}", a.display());
    let key_b = format!("local:{}", b.display());
    db.set_relation("track", "local", &key_a, "liked", true)
        .unwrap();
    db.mark_relation_synced("track", "local", &key_a, "liked")
        .unwrap();
    // 新键上已有一次取消收藏的意图（文件尚未入库也可写关系行）。
    db.set_relation("track", "local", &key_b, "liked", false)
        .unwrap();

    std::fs::rename(&a, &b).unwrap();
    db.record_scan_file(root_id, generation, &b, None, "fp-same")
        .unwrap();

    assert_eq!(
        db.relation_desired("track", "local", &key_a, "liked")
            .unwrap(),
        None,
        "旧键行删除"
    );
    assert_eq!(
        db.relation_desired("track", "local", &key_b, "liked")
            .unwrap(),
        Some(false),
        "新键既有行保留"
    );
    assert!(db.list_favorites(10).unwrap().is_empty());
    let n: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM relations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "只剩新键那一行");
}

/// 里程碑 E2：单文件删除标记（watcher Remove 事件；不删行，missing 语义与扫描一致）。
/// 播放路径入库（`add_local_file`）的行要归入所在扫描根：否则
/// `finish_scan` 的 missing 收尾永远看不到它，daemon 停机期间删掉的文件会
/// 以 missing=0 永久留在库里（AUDIT §4.3 二等行）。
#[test]
fn playback_ingested_file_joins_scan_root_and_is_marked_missing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("played.mp3");
    std::fs::write(&file, b"x").unwrap();
    let file = crate::canonical_display_path(&file);
    let mut db = LibraryDb::open_in_memory().unwrap();
    let (root_id, generation) = db.begin_scan(dir.path()).unwrap();

    db.add_local_file(&file, None).unwrap();
    let (row_root, seen): (Option<i64>, i64) = db
        .conn
        .query_row(
            "SELECT scan_root_id, last_seen_generation FROM local_files WHERE path = ?1",
            params![file.display().to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(row_root, Some(root_id), "播放入库的行应归入扫描根");
    assert_eq!(seen, generation, "应带当前代际，不被本轮收尾误判");

    // 模拟「daemon 停机期间删除」：下一轮扫描走不到它，收尾应标 missing。
    std::fs::remove_file(&file).unwrap();
    let (root_id2, generation2) = db.begin_scan(dir.path()).unwrap();
    assert_eq!(root_id2, root_id);
    assert_eq!(db.finish_scan(root_id2, generation2).unwrap(), 1);
    let missing: i64 = db
        .conn
        .query_row(
            "SELECT missing FROM local_files WHERE path = ?1",
            params![file.display().to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(missing, 1);
}

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

/// AUDIT §30.7 + §30.2.6 phase 1：专辑曲序按「碟号 → 曲号 → 标题」（碟号
/// 缺省 1、曲号缺省 999），多碟专辑不再交错；专辑按（专辑名, 专辑歌手）分组
/// ——album_artist 优先（空串视同无、回退到该行 artist），同名不同专辑歌手
/// 分两行。
#[test]
fn album_disc_order_and_album_artist() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    // 插入序刻意打乱：结果序必须来自 ORDER BY 而非 rowid。
    db.conn
        .execute_batch(
            r#"
        INSERT INTO tracks (id, source, source_key, title, album, artist, album_artist, track_number, disc_number) VALUES
          (1, 'local', 'local:/d1-02', '一-02', '精选集', '歌手甲', '专辑歌手', 2, 1),
          (2, 'local', 'local:/d1-01', '一-01', '精选集', '歌手甲', '专辑歌手', 1, 1),
          (3, 'local', 'local:/d2-01', '二-01', '精选集', '歌手乙', '专辑歌手', 1, 2),
          (4, 'local', 'local:/d2-02', '二-02', '精选集', '歌手乙', '', 2, 2),
          (5, 'local', 'local:/x-02', 'B 曲', '单曲集', 'B-singer', NULL, NULL, NULL),
          (6, 'local', 'local:/x-01', 'A 曲', '单曲集', 'A-singer', '', NULL, NULL);
        INSERT INTO local_files (track_id, path, last_seen_generation, missing, scan_root_id) VALUES
          (1, '/d1-02', 1, 0, 1),
          (2, '/d1-01', 1, 0, 1),
          (3, '/d2-01', 1, 0, 1),
          (4, '/d2-02', 1, 0, 1),
          (5, '/x-02', 1, 0, 1),
          (6, '/x-01', 1, 0, 1);
    "#,
        )
        .unwrap();

    fn album_titles(db: &mut LibraryDb, album: &str) -> Vec<String> {
        db.local_tracks_by_album(album)
            .unwrap()
            .into_iter()
            .map(|r| r.title)
            .collect()
    }
    assert_eq!(
        album_titles(&mut db, "精选集"),
        ["一-01", "一-02", "二-01", "二-02"],
        "双碟：碟 1 整碟在前，不按曲号交错"
    );
    assert_eq!(
        album_titles(&mut db, "单曲集"),
        ["A 曲", "B 曲"],
        "碟号/曲号缺省 → 标题兜底"
    );

    let albums = db.library_albums(None).unwrap();
    let group = |album: &str, artist: &str| {
        albums
            .iter()
            .find(|a| a.album == album && a.artist.as_deref() == Some(artist))
            .unwrap()
    };
    assert_eq!(
        group("精选集", "专辑歌手").track_count,
        3,
        "album_artist 相同的曲目一组"
    );
    assert_eq!(
        group("精选集", "歌手乙").track_count,
        1,
        "album_artist 为空 → 回退该行 artist，不再并进上面的组"
    );
    assert_eq!(
        group("单曲集", "B-singer").track_count,
        1,
        "无 album_artist → 该行 artist 作分组键"
    );
    assert_eq!(group("单曲集", "A-singer").track_count, 1);
    assert_eq!(
        albums.len(),
        4,
        "同名不同专辑歌手分两行（精选集 2 + 单曲集 2）"
    );
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

/// upsert 封面保护：已 rebind 的 file:// 行不被 reconcile/播放 upsert 携带的
/// 远程 URL 打回；file:// 对 file://（扫描重提取换图）仍放行更新。
#[test]
fn upsert_cover_remote_url_never_demotes_local_file() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let url = "https://y.gtimg.cn/music/photo_new/T002R300x300M000X.jpg";
    let rebound = "file://C:/Users/u/AppData/Local/hmp/covers/aa.jpg";

    // QQ 行 NULL → 远程 URL 入库（reconcile 带封面入库的正常路径）
    let id = db
        .upsert_track(&TrackRow {
            cover_uri: Some(url.into()),
            ..row()
        })
        .unwrap();
    // rebind 成本地产物
    db.rebind_cover_url(url, rebound).unwrap();
    // 再次 upsert 携带远程 URL（下一轮 reconcile）→ 不得降级
    db.upsert_track(&TrackRow {
        cover_uri: Some(url.into()),
        ..row()
    })
    .unwrap();
    let uri: String = db
        .conn
        .query_row(
            "SELECT cover_uri FROM tracks WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(uri, rebound, "file:// 不被远程 URL 覆盖");

    // 本地扫描重提取（内容哈希变 → 新 file:// 路径）→ 放行更新
    let local_a = "file://C:/Users/u/AppData/Local/hmp/covers/old.jpg";
    let local_id = db
        .upsert_track(&TrackRow {
            source: "local",
            source_key: "local:C:/m/a.flac".into(),
            title: "本地曲".into(),
            cover_uri: Some(local_a.into()),
            ..Default::default()
        })
        .unwrap();
    let newer = "file://C:/Users/u/AppData/Local/hmp/covers/bb.jpg";
    db.set_track_cover("local:C:/m/a.flac", newer).unwrap();
    let local_uri: Option<String> = db
        .conn
        .query_row(
            "SELECT cover_uri FROM tracks WHERE id = ?1",
            params![local_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(local_uri.as_deref(), Some(newer), "file:// 对 file:// 放行");

    // upsert 不带封面（播放路径 stub）→ 保留既有值（COALESCE 原语义）
    db.upsert_track(&TrackRow {
        cover_uri: None,
        ..row()
    })
    .unwrap();
    let after: String = db
        .conn
        .query_row(
            "SELECT cover_uri FROM tracks WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after, rebound, "NULL upsert 保留既有封面");
}

/// cover_cache 索引（v8）：写后可读、重复写幂等覆盖、未命中 None。
#[test]
fn cover_cache_roundtrip() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let url = "https://y.gtimg.cn/music/photo_new/T002R300x300M000X.jpg";
    assert_eq!(db.cover_cache_get(url).unwrap(), None, "未命中");

    db.cover_cache_put(url, "file://covers/aa.jpg").unwrap();
    assert_eq!(
        db.cover_cache_get(url).unwrap().as_deref(),
        Some("file://covers/aa.jpg")
    );

    // 同 URL 重复下载（不应发生，防御性）：幂等覆盖
    db.cover_cache_put(url, "file://covers/bb.jpg").unwrap();
    assert_eq!(
        db.cover_cache_get(url).unwrap().as_deref(),
        Some("file://covers/bb.jpg")
    );
    let n: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM cover_cache", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "同 URL 只占一行");
}

/// 目录封面刷新输入：`local_tracks_in_dir` 只回本目录直系文件（子目录/邻目录
/// 不算），并带出回写所需字段；查询走 `local_files.path` 唯一索引（整表扫在
/// 4k 行库上会随库规模放大，封面事件成批到达时尤其明显）。
#[test]
fn local_tracks_in_dir_scopes_to_direct_children() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = crate::canonical_display_path(tmp.path());
    let album = root.join("album");
    let other = root.join("other");
    let disc = album.join("disc1");
    std::fs::create_dir_all(&disc).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let files = [
        album.join("a.mp3"),
        album.join("b.mp3"),
        disc.join("c.mp3"),
        other.join("d.mp3"),
    ];
    // 内容各不相同：同内容同 mtime 会被 record_scan_file 判成「移动」而复用
    // 同一行（指纹命中），测不到多行。
    for (i, p) in files.iter().enumerate() {
        std::fs::write(p, format!("content-{i}")).unwrap();
    }
    let (root_id, generation) = db.begin_scan(&root).unwrap();
    for p in &files {
        let size = std::fs::metadata(p).unwrap().len();
        let fp = crate::scan::file_fingerprint(p, size).unwrap();
        db.record_scan_file(root_id, generation, p, None, &fp)
            .unwrap();
    }
    let rows = db.local_tracks_in_dir(&album).unwrap();
    let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            album.join("a.mp3").display().to_string(),
            album.join("b.mp3").display().to_string(),
        ],
        "只要直系子文件：子目录/邻目录都不算"
    );
    for row in &rows {
        assert_eq!(row.source_key, format!("local:{}", row.path));
        assert_eq!(row.cover_uri, None);
    }
    // 查询计划必须走索引（回归：改成整表 LIKE/子串匹配时这条会红）。
    let plan: String = db
        .conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT track_id FROM local_files WHERE path >= ?1 AND path < ?2",
            params!["/music/a", "/music/b"],
            |r| r.get(3),
        )
        .unwrap();
    assert!(
        plan.contains("USING") && plan.contains("INDEX") && !plan.contains("SCAN local_files"),
        "目录前缀查询应走 path 索引：{plan}"
    );
}

/// 封面回写/清空：只在值变化时写；清空只碰目标行（不误伤同专辑/同目录
/// 其它行），重复调用零写入。
#[test]
fn update_track_cover_sets_clears_and_skips_unchanged() {
    fn track_cover(db: &LibraryDb, key: &str) -> Option<String> {
        db.conn
            .query_row(
                "SELECT cover_uri FROM tracks WHERE source = 'local' AND source_key = ?1",
                params![key],
                |r| r.get(0),
            )
            .unwrap()
    }
    let mut db = LibraryDb::open_in_memory().unwrap();
    let mk = |key: &str| TrackRow {
        source: "local",
        source_key: key.into(),
        title: key.into(),
        ..Default::default()
    };
    db.upsert_track(&mk("local:/m/a.mp3")).unwrap();
    db.upsert_track(&mk("local:/m/b.mp3")).unwrap();
    assert_eq!(
        db.update_track_cover("local:/m/a.mp3", Some("file:///covers/x.jpg"))
            .unwrap(),
        1
    );
    assert_eq!(
        track_cover(&db, "local:/m/a.mp3").as_deref(),
        Some("file:///covers/x.jpg")
    );
    assert_eq!(
        db.update_track_cover("local:/m/a.mp3", Some("file:///covers/x.jpg"))
            .unwrap(),
        0,
        "值未变零写入（同目录多首曲目共享同一 URI）"
    );
    assert_eq!(
        track_cover(&db, "local:/m/b.mp3"),
        None,
        "同专辑其它行不跟着写"
    );
    assert_eq!(
        db.update_track_cover("local:/m/b.mp3", None).unwrap(),
        0,
        "本来就是空：清空零写入"
    );
    assert_eq!(db.update_track_cover("local:/m/a.mp3", None).unwrap(), 1);
    assert_eq!(track_cover(&db, "local:/m/a.mp3"), None);
    assert_eq!(
        db.update_track_cover("local:/m/a.mp3", None).unwrap(),
        0,
        "重复清空零写入"
    );
    assert_eq!(
        db.update_track_cover("local:/m/nope.mp3", Some("file:///x.jpg"))
            .unwrap(),
        0,
        "库中无此曲目：零行"
    );
}

/// v9 迁移：v8 库原地升级——`user_version` 变 9、旧数据保留、新列按默认
/// （mid 为 NULL、tag_version 为 0），重复打开幂等。
#[test]
fn migration_v8_to_v9_upgrades_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.sqlite3");
    // 按 v1→v8 逐级建一个真实的 v8 库（v5/v7 是数据迁移、不改 schema，跳过）。
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        for step in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V6,
            MIGRATION_V8,
        ] {
            conn.execute_batch(step).unwrap();
        }
        conn.pragma_update(None, "user_version", 8).unwrap();
        conn.execute(
            "INSERT INTO tracks (source, source_key, title, album, artist)
             VALUES ('qq', 'mid-old', '旧曲', '旧专辑', '旧歌手')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO track_artists (track_id, artist, position) VALUES (1, '旧歌手', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO local_files (track_id, path) VALUES (1, '/old.mp3')",
            [],
        )
        .unwrap();
    }
    {
        let db = LibraryDb::open(&path).unwrap();
        assert_eq!(db.version().unwrap(), 9);
        let (title, album_mid): (String, Option<String>) = db
            .conn
            .query_row(
                "SELECT title, album_mid FROM tracks WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "旧曲", "旧数据保留");
        assert_eq!(album_mid, None, "旧行不回填 mid");
        let artist_mid: Option<String> = db
            .conn
            .query_row(
                "SELECT artist_mid FROM track_artists WHERE track_id = 1 AND position = 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(artist_mid, None);
        let tag_version: i64 = db
            .conn
            .query_row(
                "SELECT tag_version FROM local_files WHERE track_id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tag_version, 0, "旧行等下次扫描升级");
    }
    // 重复打开：版本不倒退、数据不丢（迁移幂等）。
    let db = LibraryDb::open(&path).unwrap();
    assert_eq!(db.version().unwrap(), 9);
    let n: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

/// v9：QQ 行 upsert 落 `album_mid` 与 `artist_mid`；窄投影（无 mid）不覆盖
/// 旧 mid，也不清空既有歌手行。
#[test]
fn upsert_track_persists_mids_and_keeps_them_on_null_projection() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    let full = TrackRow {
        source: "qq",
        source_key: "mid-1".into(),
        title: "夜曲".into(),
        album: Some("十一月的萧邦".into()),
        album_mid: Some("album-mid".into()),
        artist: Some("周杰伦 / 费玉清".into()),
        artist_refs: vec![
            ArtistRefRow {
                mid: Some("singer-1".into()),
                name: "周杰伦".into(),
            },
            ArtistRefRow {
                mid: Some("singer-2".into()),
                name: "费玉清".into(),
            },
        ],
        ..Default::default()
    };
    let tid = db.upsert_track(&full).unwrap();
    assert_eq!(db.album_mid(tid).unwrap().as_deref(), Some("album-mid"));
    assert_eq!(
        db.track_artists(tid).unwrap(),
        vec![
            ArtistRefRow {
                mid: Some("singer-1".into()),
                name: "周杰伦".into()
            },
            ArtistRefRow {
                mid: Some("singer-2".into()),
                name: "费玉清".into()
            },
        ]
    );

    // 窄投影：title/artist 无 mid、无歌手行 → 身份列与歌手行原样保留。
    let narrow = TrackRow {
        source: "qq",
        source_key: "mid-1".into(),
        title: "夜曲".into(),
        artist: Some("周杰伦 / 费玉清".into()),
        ..Default::default()
    };
    db.upsert_track(&narrow).unwrap();
    assert_eq!(db.album_mid(tid).unwrap().as_deref(), Some("album-mid"));
    assert_eq!(db.track_artists(tid).unwrap().len(), 2);

    // 歌手投影显式给 NULL mid 但同名 → 按同名旧行保留 mid（COALESCE 语义）。
    let reordered = TrackRow {
        source: "qq",
        source_key: "mid-1".into(),
        title: "夜曲".into(),
        artist_refs: vec![
            ArtistRefRow {
                mid: None,
                name: "费玉清".into(),
            },
            ArtistRefRow {
                mid: None,
                name: "周杰伦".into(),
            },
        ],
        ..Default::default()
    };
    db.upsert_track(&reordered).unwrap();
    assert_eq!(
        db.track_artists(tid).unwrap(),
        vec![
            ArtistRefRow {
                mid: Some("singer-2".into()),
                name: "费玉清".into()
            },
            ArtistRefRow {
                mid: Some("singer-1".into()),
                name: "周杰伦".into()
            },
        ],
        "重排/无 mid 的投影不得丢身份"
    );
}

/// v9：批量缓存路径同样落 mid（列表解析缓存不再只留名字）。
#[test]
fn upsert_tracks_batch_persists_mids() {
    let mut db = LibraryDb::open_in_memory().unwrap();
    db.upsert_tracks_batch(&[TrackRow {
        source: "qq",
        source_key: "mid-batch".into(),
        title: "批量曲".into(),
        album: Some("批量专辑".into()),
        album_mid: Some("album-batch".into()),
        artist: Some("A / B".into()),
        artist_refs: vec![
            ArtistRefRow {
                mid: Some("sa".into()),
                name: "A".into(),
            },
            ArtistRefRow {
                mid: Some("sb".into()),
                name: "B".into(),
            },
        ],
        ..Default::default()
    }])
    .unwrap();
    let tid = db.track_id("qq", "mid-batch").unwrap().unwrap();
    assert_eq!(db.album_mid(tid).unwrap().as_deref(), Some("album-batch"));
    assert_eq!(db.track_artists(tid).unwrap().len(), 2);
}

/// §30.2.6 phase 1：专辑组行透传 `album_mid`（QQ 行回填后），本地组为 NULL；
/// 同专辑同专辑歌手的多首曲目仍是一行。
#[test]
fn library_albums_exposes_album_mid_per_group() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let (root_id, generation) = db.begin_scan(dir.path()).unwrap();
    let meta = |title: &str, album_artist: &str| crate::local::LocalMeta {
        title: title.into(),
        artist: Some(album_artist.into()),
        album: Some("同名专辑".into()),
        album_artist: Some(album_artist.into()),
        ..Default::default()
    };
    let mut ids = Vec::new();
    for (name, title, album_artist, fp) in [
        ("a.mp3", "A", "甲", "fp-album-a"),
        ("b.mp3", "B", "甲", "fp-album-b"),
        ("c.mp3", "C", "乙", "fp-album-c"),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, name.as_bytes()).unwrap();
        db.record_scan_file(
            root_id,
            generation,
            &path,
            Some(&meta(title, album_artist)),
            fp,
        )
        .unwrap();
        ids.push(
            db.track_id(
                "local",
                &format!("local:{}", crate::canonical_display_path(&path).display()),
            )
            .unwrap()
            .unwrap(),
        );
    }
    // 模拟 reconcile 回填的专辑 mid（本地行本身没有 mid）。
    db.conn
        .execute(
            "UPDATE tracks SET album_mid = 'amid-jia' WHERE id = ?1",
            params![ids[0]],
        )
        .unwrap();

    let albums = db.library_albums(None).unwrap();
    assert_eq!(albums.len(), 2, "两个专辑歌手 → 两行");
    let jia = albums
        .iter()
        .find(|a| a.artist.as_deref() == Some("甲"))
        .unwrap();
    assert_eq!(jia.track_count, 2, "同专辑歌手两首仍一行");
    assert_eq!(jia.album_mid.as_deref(), Some("amid-jia"));
    let yi = albums
        .iter()
        .find(|a| a.artist.as_deref() == Some("乙"))
        .unwrap();
    assert_eq!(yi.track_count, 1);
    assert_eq!(yi.album_mid, None);
}

/// §30.2.6 phase 1：旧解析规则的行（tag_version=0）即使 mtime/size 未变，
/// 扫描也强制重读标签 → Updated + 字段刷新 + 版本推进；再扫一次 → Skipped。
#[test]
fn scan_rereads_tags_when_tag_version_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.flac");
    std::fs::write(&path, b"x").unwrap();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let (root_id, generation) = db.begin_scan(dir.path()).unwrap();
    // 旧规则入库的形态：行有文件指纹，但标签列为空、版本 0。
    assert!(matches!(
        db.record_scan_file(root_id, generation, &path, None, "fp-tag")
            .unwrap(),
        ScanOutcome::Added
    ));
    db.conn
        .execute(
            "UPDATE local_files SET tag_version = 0 WHERE path = ?1",
            params![path.display().to_string()],
        )
        .unwrap();
    let meta = crate::local::LocalMeta {
        title: "夜曲".into(),
        artist: Some("周杰伦".into()),
        album: Some("十一月的萧邦".into()),
        duration_ms: Some(193_000),
        ..Default::default()
    };
    // mtime/size 与文件一致，仅解析规则版本过期 → 强制重读。
    assert!(
        matches!(
            db.record_scan_file(root_id, generation, &path, Some(&meta), "fp-tag")
                .unwrap(),
            ScanOutcome::Updated
        ),
        "旧 tag_version 必须触发重读，而不是 Skipped"
    );
    let (title, artist, album, version): (String, Option<String>, Option<String>, i64) = db
        .conn
        .query_row(
            "SELECT t.title, t.artist, t.album, lf.tag_version
             FROM tracks t JOIN local_files lf ON lf.track_id = t.id WHERE lf.path = ?1",
            params![path.display().to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(title, "夜曲");
    assert_eq!(artist.as_deref(), Some("周杰伦"));
    assert_eq!(album.as_deref(), Some("十一月的萧邦"));
    assert_eq!(version, LOCAL_TAG_VERSION);
    // 版本已是最新、文件未变 → 增量跳过。
    assert!(matches!(
        db.record_scan_file(root_id, generation, &path, Some(&meta), "fp-tag")
            .unwrap(),
        ScanOutcome::Skipped
    ));
    // 文件变化（size/mtime 变）→ 常规更新路径同样把版本推进到当前。
    std::fs::write(&path, b"changed-content").unwrap();
    let changed = crate::local::LocalMeta {
        title: "换标签".into(),
        ..meta
    };
    assert!(matches!(
        db.record_scan_file(root_id, generation, &path, Some(&changed), "fp-tag-2")
            .unwrap(),
        ScanOutcome::Updated
    ));
    let (title, version): (String, i64) = db
        .conn
        .query_row(
            "SELECT t.title, lf.tag_version
             FROM tracks t JOIN local_files lf ON lf.track_id = t.id WHERE lf.path = ?1",
            params![path.display().to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(title, "换标签");
    assert_eq!(version, LOCAL_TAG_VERSION);
}

/// §30.2.6 phase 1：播放路径入库（`add_local_file`）同样写当前标签版本，
/// 避免「先播放后扫描」的行在下次扫描被误判为旧规则。
#[test]
fn add_local_file_writes_current_tag_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("played.mp3");
    std::fs::write(&path, b"x").unwrap();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let meta = crate::local::LocalMeta {
        title: "先播后扫".into(),
        artist: Some("歌手".into()),
        ..Default::default()
    };
    db.add_local_file(&path, Some(&meta)).unwrap();
    let version: i64 = db
        .conn
        .query_row(
            "SELECT tag_version FROM local_files WHERE path = ?1",
            params![path.display().to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version, LOCAL_TAG_VERSION);
}

/// §30.2.6 phase 1：本地标签多歌手的展示串也走统一 `" / "`（播放入库与
/// 扫描入库同一口径），歌手行 mid 恒 NULL。
#[test]
fn local_artist_display_uses_shared_separator() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let meta = crate::local::LocalMeta {
        title: "多歌手".into(),
        artist: Some("张韶涵".into()),
        artists: vec!["张韶涵".into(), "HOYO-MiX".into()],
        album: Some("专辑".into()),
        ..Default::default()
    };
    let played = dir.path().join("played.mp3");
    std::fs::write(&played, b"a").unwrap();
    let played_id = db.add_local_file(&played, Some(&meta)).unwrap();
    let scanned = dir.path().join("scanned.mp3");
    std::fs::write(&scanned, b"b").unwrap();
    let (root_id, generation) = db.begin_scan(dir.path()).unwrap();
    db.record_scan_file(root_id, generation, &scanned, Some(&meta), "fp-local")
        .unwrap();
    let expected_refs = vec![
        ArtistRefRow {
            mid: None,
            name: "张韶涵".into(),
        },
        ArtistRefRow {
            mid: None,
            name: "HOYO-MiX".into(),
        },
    ];
    for (label, tid) in [
        ("播放入库", played_id),
        (
            "扫描入库",
            db.track_id(
                "local",
                &format!(
                    "local:{}",
                    crate::canonical_display_path(&scanned).display()
                ),
            )
            .unwrap()
            .unwrap(),
        ),
    ] {
        let artist: Option<String> = db
            .conn
            .query_row(
                "SELECT artist FROM tracks WHERE id = ?1",
                params![tid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(artist.as_deref(), Some("张韶涵 / HOYO-MiX"), "{label}");
        assert_eq!(db.track_artists(tid).unwrap(), expected_refs, "{label}");
    }
}
