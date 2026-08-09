//! `hmp playlist`：本地歌单管理（直读媒体库）。
//!
//! 用法：
//! ```text
//! hmp playlist list                          # 列出歌单
//! hmp playlist create <名称>                 # 新建
//! hmp playlist rename <id> <名称>            # 重命名
//! hmp playlist delete <id>                   # 删除
//! hmp playlist add <id> <track-id>           # 追加曲目（QQ mid 或 local:<path>）
//! hmp playlist remove <id> <序号>            # 按序号移除曲目
//! hmp playlist show <id>                     # 查看歌单内曲目
//! ```

use std::io::Write;

use super::library::{open_library, provider_of};

/// 新建歌单，打印 id。
pub async fn create(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = open_library()?;
    let id = db.create_playlist(name)?;
    println!("已创建歌单 #{id}: {name}");
    Ok(())
}

/// 重命名。
pub async fn rename(id: i64, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = open_library()?;
    match db.rename_playlist(id, name) {
        Ok(()) => {
            println!("已重命名 #{id}: {name}");
            Ok(())
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(format!("歌单不存在: #{id}").into()),
        Err(e) => Err(e.into()),
    }
}

/// 删除歌单。
pub async fn delete(id: i64) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = open_library()?;
    db.delete_playlist(id)?;
    println!("已删除歌单 #{id}");
    Ok(())
}

/// 追加曲目（幂等）。
pub async fn add(id: i64, track: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = open_library()?;
    let (source, source_key) = provider_of(track);
    match db.add_playlist_track(id, source, &source_key, track) {
        Ok(()) => {
            println!("已加入歌单 #{id}: {track}");
            Ok(())
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(format!("歌单不存在: #{id}").into()),
        Err(e) => Err(e.into()),
    }
}

/// 按序号移除曲目。
pub async fn remove_track(id: i64, position: i64) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = open_library()?;
    db.remove_playlist_track(id, position)?;
    println!("已从歌单 #{id} 移除序号 {position}");
    Ok(())
}

/// 歌单列表。
pub async fn list() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = open_library()?;
    let rows = db.list_playlists()?;
    let mut stdout = std::io::stdout().lock();
    if rows.is_empty() {
        writeln!(stdout, "暂无本地歌单（hmp playlist create <名称>）")?;
    } else {
        for p in &rows {
            writeln!(stdout, "#{}  {}  ({} 首)", p.id, p.name, p.track_count)?;
        }
    }
    stdout.flush()?;
    Ok(())
}

/// 歌单内曲目。
pub async fn show(id: i64) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = open_library()?;
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
