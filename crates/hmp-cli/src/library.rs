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
    println!("已触发 QQ 媒体库同步");
    // 轮询本地 outbox 直至空闲（reconcile 写入 synced 事实，pending 为空）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let pending = {
            let Ok(mut db) = super::library::open_library() else {
                break;
            };
            let rels = db.relations_pending().unwrap_or_default();
            let ops = db.playlist_ops_pending().unwrap_or_default();
            rels.len() + ops.len()
        };
        if pending == 0 {
            println!("媒体库已同步");
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            println!("同步超时（仍有 {pending} 条待同步意图，稍后自动重试）");
            return Ok(());
        }
    }
    Ok(())
}

/// 待同步意图与错误（直读本地 DB）。
pub async fn sync_status() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rels = db.relations_pending()?;
    let pls = db.playlists_pending()?;
    let ops = db.playlist_ops_pending()?;
    let mut stdout = std::io::stdout().lock();
    let total = rels.len() + pls.len() + ops.len();
    if total == 0 {
        writeln!(stdout, "媒体库已同步（无待处理意图）")?;
    } else {
        writeln!(stdout, "待同步意图: {} 条", total)?;
        for r in rels.iter().filter(|r| r.sync_state == "error") {
            writeln!(
                stdout,
                "  错误: {}/{} {} (重试 {} 次: {})",
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
                "  错误: 歌单 #{} {} (重试 {} 次: {})",
                p.id,
                p.name,
                p.retry_count,
                p.last_sync_error.as_deref().unwrap_or("")
            )?;
        }
        for o in ops.iter().filter(|o| o.sync_state == "error") {
            writeln!(
                stdout,
                "  错误: 歌单操作 #{} {} (重试 {} 次: {})",
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

/// 我喜欢的歌曲（本地事实视图）。
pub async fn tracks_liked() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rows = db.list_favorites(10_000)?;
    let mut stdout = std::io::stdout().lock();
    if rows.is_empty() {
        writeln!(
            stdout,
            "暂无收藏（hmp favorite add <track-id> 或 hmp library sync）"
        )?;
    } else {
        for (i, r) in rows.iter().enumerate() {
            writeln!(stdout, "{:>3}. {}  {}", i + 1, r.title, r.source_key)?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// 我收藏的专辑（本地事实视图；标题随 sync 补齐前显示 id）。
pub async fn albums_liked() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rows = db.relation_rows("album", "liked")?;
    let mut stdout = std::io::stdout().lock();
    if rows.is_empty() {
        writeln!(stdout, "暂无收藏专辑（hmp library sync）")?;
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
