//! `hmp library`：媒体库查询与 QQ 同步（直读本地 DB + daemon reconcile 触发）。
//!
//! ```text
//! hmp library history                    # 最近播放
//! hmp library sync                       # 从 QQ 拉用户库快照 reconcile
//! hmp library sync-status                # 待同步意图/错误
//! hmp library tracks --liked             # 我喜欢的歌曲（本地事实视图）
//! hmp library albums --liked             # 我收藏的专辑
//! ```

use std::io::Write;

use hmp_core::Request;

use super::client::DaemonClient;
use super::commands;

/// 打开媒体库（不存在则创建；WAL 模式与 daemon 并发安全）。
pub fn open_library() -> Result<hmp_storage::LibraryDb, Box<dyn std::error::Error>> {
    let path = hmp_storage::data_dir().join("library.sqlite3");
    Ok(hmp_storage::LibraryDb::open(&path)?)
}

/// track id → (source, source_key)：与引擎 `track_row` 同一规则
/// （`local:` 前缀 → local，其余 → qq），保证收藏/歌单与播放历史一致。
pub fn provider_of(id: &str) -> (&'static str, String) {
    if hmp_core::TrackProvider::from_id(id) == hmp_core::TrackProvider::Local {
        ("local", id.to_string())
    } else {
        ("qq", id.to_string())
    }
}

/// 触发 reconcile 并等待 outbox 消化（60s 超时）。
pub async fn sync() -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    commands::cmd_simple(&mut c, Request::LibrarySync).await?;
    println!("QQ library sync triggered");
    // 轮询本地 outbox 直至空闲（reconcile 写入 synced 事实，pending 为空）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let pending = {
            let Ok(mut db) = super::library::open_library() else {
                break;
            };
            let rels = db.relations_pending().unwrap_or_default();
            let pls = db.playlists_pending().unwrap_or_default();
            let ops = db.playlist_ops_pending().unwrap_or_default();
            rels.len() + pls.len() + ops.len()
        };
        if pending == 0 {
            println!("Library synced");
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            println!("Sync timed out ({pending} intents still pending; will retry automatically)");
            return Ok(());
        }
    }
    Ok(())
}

/// 通用窗口边界：`(start, end)`（`limit == 0` 表示到末尾；start 钳制到 total）。
pub(crate) fn window_bounds(total: usize, offset: usize, limit: usize) -> (usize, usize) {
    let start = offset.min(total);
    let end = if limit == 0 {
        total
    } else {
        (start + limit).min(total)
    };
    (start, end)
}

/// 待同步意图总数（sync_status 文本/JSON 两路共用）。
fn total_pending(
    rels: &[hmp_storage::RelationRow],
    pls: &[hmp_storage::PlaylistRow],
    ops: &[hmp_storage::PlaylistOpRow],
) -> usize {
    rels.len() + pls.len() + ops.len()
}

/// 待同步意图与错误（直读本地 DB）。
pub async fn sync_status(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rels = db.relations_pending()?;
    let pls = db.playlists_pending()?;
    let ops = db.playlist_ops_pending()?;
    if json {
        let errors: Vec<serde_json::Value> = rels
            .iter()
            .filter(|r| r.sync_state == "error")
            .map(|r| {
                serde_json::json!({
                    "kind": "relation",
                    "entity_type": r.entity_type,
                    "relation": r.relation,
                    "entity_key": r.entity_key,
                    "retry_count": r.retry_count,
                    "error": r.last_sync_error,
                })
            })
            .chain(pls.iter().filter(|p| p.sync_state == "error").map(|p| {
                serde_json::json!({
                    "kind": "playlist",
                    "id": p.id,
                    "name": p.name,
                    "retry_count": p.retry_count,
                    "error": p.last_sync_error,
                })
            }))
            .chain(ops.iter().filter(|o| o.sync_state == "error").map(|o| {
                serde_json::json!({
                    "kind": "playlist_op",
                    "id": o.id,
                    "op": o.op,
                    "retry_count": o.retry_count,
                    "error": o.last_error,
                })
            }))
            .collect();
        return super::output::print(&serde_json::json!({
            "pending": total_pending(&rels, &pls, &ops),
            "errors": errors,
        }));
    }
    let mut stdout = std::io::stdout().lock();
    let total = rels.len() + pls.len() + ops.len();
    if total == 0 {
        writeln!(stdout, "Library synced (no pending intents)")?;
    } else {
        writeln!(stdout, "Pending sync intents: {total}")?;
        for r in rels.iter().filter(|r| r.sync_state == "error") {
            writeln!(
                stdout,
                "  error: {}/{} {} (retried {} times: {})",
                r.entity_type,
                r.relation,
                r.entity_key,
                r.retry_count,
                r.last_sync_error.as_deref().unwrap_or("")
            )?;
        }
        for p in pls.iter().filter(|p| p.sync_state == "error") {
            writeln!(
                stdout,
                "  error: playlist #{} {} (retried {} times: {})",
                p.id,
                p.name,
                p.retry_count,
                p.last_sync_error.as_deref().unwrap_or("")
            )?;
        }
        for o in ops.iter().filter(|o| o.sync_state == "error") {
            writeln!(
                stdout,
                "  error: playlist op #{} {} (retried {} times: {})",
                o.id,
                o.op,
                o.retry_count,
                o.last_error.as_deref().unwrap_or("")
            )?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// 我喜欢的歌曲（本地事实视图；`offset` 0 基起始，`limit` 0 = 全量）。
pub async fn tracks_liked(
    offset: usize,
    limit: usize,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rows = db.list_favorites(10_000)?;
    let (start, end) = window_bounds(rows.len(), offset, limit);
    if json {
        let items: Vec<serde_json::Value> = rows[start..end]
            .iter()
            .map(|r| {
                serde_json::json!({
                    "track_id": r.track_id,
                    "source": r.source,
                    "source_key": r.source_key,
                    "title": r.title,
                })
            })
            .collect();
        return super::output::print(&serde_json::json!({
            "total": rows.len(), "offset": start, "items": items,
        }));
    }
    let mut stdout = std::io::stdout().lock();
    if rows.is_empty() {
        writeln!(
            stdout,
            "No favorites yet (try `hmp favorite add <track-id>` or `hmp library sync`)"
        )?;
    } else {
        for (i, r) in rows[start..end].iter().enumerate() {
            writeln!(stdout, "{:>3}. {}  {}", start + i + 1, r.title, r.source_key)?;
        }
        if end < rows.len() {
            writeln!(stdout, "（{end} / {} 首：--offset {end} 翻页）", rows.len())?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// 本地曲目浏览（里程碑 E）：默认全部本地曲目，支持搜索/歌手/专辑/收藏过滤；
/// `offset` 0 基起始，`limit` 0 = 全量。
pub async fn tracks_local(
    search: Option<&str>,
    artist: Option<&str>,
    album: Option<&str>,
    liked: bool,
    offset: usize,
    limit: usize,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rows = db.library_tracks(search, artist, album, liked)?;
    let (start, end) = window_bounds(rows.len(), offset, limit);
    if json {
        let items: Vec<serde_json::Value> = rows[start..end]
            .iter()
            .map(|r| {
                serde_json::json!({
                    "track_id": r.track_id,
                    "source_key": r.source_key,
                    "title": r.title,
                    "artist": r.artist,
                    "album": r.album,
                    "duration_ms": r.duration_ms,
                    "year": r.year,
                    "genre": r.genre,
                    "missing": r.missing,
                })
            })
            .collect();
        return super::output::print(&serde_json::json!({
            "total": rows.len(), "offset": start, "items": items,
        }));
    }
    let mut stdout = std::io::stdout().lock();
    if rows.is_empty() {
        writeln!(
            stdout,
            "no matching local tracks (run `hmp library scan <dir>` first)"
        )?;
    } else {
        for (i, r) in rows[start..end].iter().enumerate() {
            let missing = if r.missing { " [missing]" } else { "" };
            writeln!(
                stdout,
                "{:>3}. {}{}  {}",
                start + i + 1,
                r.title,
                missing,
                r.source_key
            )?;
        }
        if end < rows.len() {
            writeln!(stdout, "（{end} / {} 首：--offset {end} 翻页）", rows.len())?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// 本地专辑聚合（里程碑 E）。
pub async fn albums_local(search: Option<&str>, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rows = db.library_albums(search)?;
    if json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|g| {
                serde_json::json!({
                    "album": g.album,
                    "artist": g.artist,
                    "track_count": g.track_count,
                })
            })
            .collect();
        return super::output::print(&serde_json::json!({ "total": items.len(), "items": items }));
    }
    let mut stdout = std::io::stdout().lock();
    if rows.is_empty() {
        writeln!(
            stdout,
            "no local albums (run `hmp library scan <dir>` first)"
        )?;
    } else {
        for (i, g) in rows.iter().enumerate() {
            writeln!(
                stdout,
                "{:>3}. {}  {} ({} tracks)",
                i + 1,
                g.album,
                g.artist.as_deref().unwrap_or("(unknown artist)"),
                g.track_count
            )?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// 本地歌手聚合（里程碑 E；多艺术家拆行）。
pub async fn artists_local(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rows = db.library_artists()?;
    if json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|g| {
                serde_json::json!({
                    "artist": g.artist,
                    "track_count": g.track_count,
                })
            })
            .collect();
        return super::output::print(&serde_json::json!({ "total": items.len(), "items": items }));
    }
    let mut stdout = std::io::stdout().lock();
    if rows.is_empty() {
        writeln!(
            stdout,
            "no local artists (run `hmp library scan <dir>` first)"
        )?;
    } else {
        for (i, g) in rows.iter().enumerate() {
            writeln!(
                stdout,
                "{:>3}. {} ({} tracks)",
                i + 1,
                g.artist,
                g.track_count
            )?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// 我收藏的专辑（本地事实视图；标题随 sync 补齐前显示 id）。
pub async fn albums_liked(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rows = db.relation_rows("album", "liked")?;
    if json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "entity_key": r.entity_key,
                    "sync_state": r.sync_state,
                })
            })
            .collect();
        return super::output::print(&serde_json::json!({ "total": items.len(), "items": items }));
    }
    let mut stdout = std::io::stdout().lock();
    if rows.is_empty() {
        writeln!(stdout, "No liked albums yet (try `hmp library sync`)")?;
    } else {
        for (i, r) in rows.iter().enumerate() {
            writeln!(
                stdout,
                "{:>3}. album {}  {}",
                i + 1,
                r.entity_key,
                r.sync_state
            )?;
        }
    }
    stdout.flush()?;
    Ok(())
}
