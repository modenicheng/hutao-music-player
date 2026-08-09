//! `hmp playlist`：本地歌单管理。
//!
//! 写操作走 daemon（local 直接生效；QQ owned 经 outbox 异步同步，spec §5）；
//! 列表/详情直读本地媒体库。
//!
//! ```text
//! hmp playlist list [--scope all|local|owned|favorite]   # 列出歌单
//! hmp playlist create <名称>                             # 新建（本地）
//! hmp playlist rename <id> <名称>                        # 重命名
//! hmp playlist delete <id>                               # 删除
//! hmp playlist add <id> <track-id>                       # 追加曲目
//! hmp playlist remove <id> <序号>                        # 按序号移除曲目
//! hmp playlist show <id>                                 # 查看歌单内曲目
//! ```

use std::io::Write;

use hmp_core::{PlaylistWriteOp, Request};

use super::client::DaemonClient;
use super::commands;
use super::library::provider_of;

/// 新建歌单（本地；返回 id）。
pub async fn create(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    let resp = commands::send(
        &mut c,
        Request::PlaylistWrite {
            op: PlaylistWriteOp::Create {
                name: name.to_string(),
            },
        },
    )
    .await?;
    match resp {
        hmp_core::Response::Created(id) => {
            println!("已创建歌单 #{id}: {name}");
            Ok(())
        }
        hmp_core::Response::Err { code, message } => {
            Err(format!("创建失败({code:?}): {message}").into())
        }
        _ => Err("创建响应异常".into()),
    }
}

/// 重命名（local 直接生效；QQ owned 不支持远端重命名）。
pub async fn rename(id: i64, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    commands::cmd_simple(
        &mut c,
        Request::PlaylistWrite {
            op: PlaylistWriteOp::Rename {
                id,
                name: name.to_string(),
            },
        },
    )
    .await?;
    println!("已重命名 #{id}: {name}");
    Ok(())
}

/// 删除（local 立即；QQ owned 本地行保留到远端删除成功）。
pub async fn delete(id: i64) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    commands::cmd_simple(
        &mut c,
        Request::PlaylistWrite {
            op: PlaylistWriteOp::Delete { id },
        },
    )
    .await?;
    println!("已删除歌单 #{id}");
    Ok(())
}

/// 追加曲目（幂等；owned 歌单同步到 QQ）。
pub async fn add(id: i64, track: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    let (source, key) = provider_of(track);
    commands::cmd_simple(
        &mut c,
        Request::PlaylistWrite {
            op: PlaylistWriteOp::AddTrack {
                id,
                source: source.to_string(),
                key: key.clone(),
                title: track.to_string(),
            },
        },
    )
    .await?;
    println!("已加入歌单 #{id}: {track}");
    Ok(())
}

/// 按序号移除曲目（owned 歌单同步到 QQ）。
pub async fn remove_track(id: i64, position: i64) -> Result<(), Box<dyn std::error::Error>> {
    let mut c = DaemonClient::connect_or_spawn().await?;
    commands::cmd_simple(
        &mut c,
        Request::PlaylistWrite {
            op: PlaylistWriteOp::RemoveTrack { id, position },
        },
    )
    .await?;
    println!("已从歌单 #{id} 移除序号 {position}");
    Ok(())
}

/// 歌单列表（统一视图：local / qq-owned / qq-favorite）。
pub async fn list() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let rows = db.list_playlists()?;
    let mut stdout = std::io::stdout().lock();
    if rows.is_empty() {
        writeln!(stdout, "暂无本地歌单（hmp playlist create <名称>）")?;
    } else {
        writeln!(stdout, "{:<5} {:<12} {:<12} NAME", "ID", "TYPE", "SYNC")?;
        for p in &rows {
            let type_name = match p.relation.as_str() {
                "local" => "local",
                "owned" => "qq-owned",
                _ => "qq-fav",
            };
            writeln!(
                stdout,
                "{:<5} {:<12} {:<12} {}  ({} 首)",
                format!("#{}", p.id),
                type_name,
                p.sync_state,
                p.name,
                p.track_count
            )?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// 歌单内曲目。
pub async fn show(id: i64) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = super::library::open_library()?;
    let tracks = db.playlist_tracks(id)?;
    let mut stdout = std::io::stdout().lock();
    if tracks.is_empty() {
        writeln!(stdout, "歌单 #{id} 为空")?;
    } else {
        for t in &tracks {
            writeln!(stdout, "{:>3}. {}  {}", t.position, t.title, t.source_key)?;
        }
    }
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use hmp_storage::LibraryDb;

    #[test]
    fn playlist_crud_roundtrip() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let id = db.create_playlist("驾车").unwrap();
        assert_eq!(db.list_playlists().unwrap().len(), 1);

        db.add_playlist_track(id, "qq", "mid-a", "mid-a").unwrap();
        db.add_playlist_track(id, "local", "local:/m/a.flac", "a.flac")
            .unwrap();
        // 幂等：重复曲目不重复加入。
        db.add_playlist_track(id, "qq", "mid-a", "mid-a").unwrap();

        let rows = db.playlist_tracks(id).unwrap();
        assert_eq!(rows.len(), 2, "重复曲目应去重");
        assert_eq!(rows[0].position, 0);
        assert_eq!(rows[1].position, 1);
        assert_eq!(db.list_playlists().unwrap()[0].track_count, 2);

        db.remove_playlist_track(id, 1).unwrap();
        assert_eq!(db.playlist_tracks(id).unwrap().len(), 1);

        db.rename_playlist(id, "夜路").unwrap();
        assert_eq!(db.list_playlists().unwrap()[0].name, "夜路");
        assert!(db.rename_playlist(999, "x").is_err(), "不存在歌单应报错");

        db.delete_playlist(id).unwrap();
        assert!(db.list_playlists().unwrap().is_empty());
    }
}
