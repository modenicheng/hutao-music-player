//! `hmp favorite`：收藏管理。
//!
//! 写操作（add/remove）走 daemon（本地先提交 + QQ 异步同步，spec §3.3）；
//! 列表直读本地媒体库。
//!
//! ```text
//! hmp favorite list                # 列出收藏
//! hmp favorite add <track-id>      # 收藏（QQ mid 或 local:<path>）
//! hmp favorite remove <track-id>   # 取消收藏
//! ```

use std::io::Write;

use hmp_core::Request;

use super::client::DaemonClient;
use super::commands;
use super::library::provider_of;

/// 收藏（本地先提交；QQ 由 daemon SyncWorker 异步同步）。
pub async fn add(id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    let (source, key) = provider_of(id);
    commands::cmd_simple(
        &mut c,
        Request::Favorite {
            source: source.to_string(),
            key: key.clone(),
            title: id.to_string(),
            desired: true,
        },
    )
    .await?;
    println!("已收藏: {id}");
    Ok(())
}

/// 取消收藏（本地先提交；unlike 由 SyncWorker 同步）。
pub async fn remove(id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    let (source, key) = provider_of(id);
    commands::cmd_simple(
        &mut c,
        Request::Favorite {
            source: source.to_string(),
            key: key.clone(),
            title: id.to_string(),
            desired: false,
        },
    )
    .await?;
    println!("已取消收藏: {id}");
    Ok(())
}

/// 列出收藏（本地事实视图，直读媒体库）。
pub async fn list() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rows = db.list_favorites(100)?;
    let mut stdout = std::io::stdout().lock();
    if rows.is_empty() {
        writeln!(stdout, "暂无收藏（hmp favorite add <track-id>）")?;
    } else {
        for (i, r) in rows.iter().enumerate() {
            writeln!(stdout, "{:>2}. {}  {}", i + 1, r.title, r.source_key)?;
        }
    }
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::provider_of;
    use hmp_storage::LibraryDb;

    #[test]
    fn provider_of_maps_local_and_qq() {
        let (s, k) = provider_of("local:/home/u/music/a.flac");
        assert_eq!(s, "local");
        assert_eq!(k, "local:/home/u/music/a.flac");
        let (s, k) = provider_of("003aQm4F3GJHZq");
        assert_eq!(s, "qq");
        assert_eq!(k, "003aQm4F3GJHZq");
    }

    #[test]
    fn favorite_add_list_remove_roundtrip() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let tid = db.add_favorite("qq", "mid-1", "mid-1").unwrap();
        assert!(db.is_favorite(tid).unwrap());
        let rows = db.list_favorites(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source_key, "mid-1");
        // 幂等：重复收藏不报错。
        db.add_favorite("qq", "mid-1", "mid-1").unwrap();
        assert_eq!(db.list_favorites(10).unwrap().len(), 1);
        db.remove_favorite(tid).unwrap();
        assert!(!db.is_favorite(tid).unwrap());
        assert!(db.list_favorites(10).unwrap().is_empty());
    }
}
