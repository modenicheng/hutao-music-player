//! 媒体库 CLI 共享辅助（`hmp favorite` / `hmp playlist`）。
//!
//! 与 `hmp history`/`hmp scan` 一致：CLI 直连本地 SQLite（WAL 多进程安全），
//! 不经过 daemon IPC——播放记录由 daemon 写入、管理命令由 CLI 直读。

use hmp_core::TrackProvider;
use hmp_storage::LibraryDb;

/// 打开媒体库（不存在则创建；WAL 模式与 daemon 并发安全）。
pub fn open_library() -> Result<LibraryDb, Box<dyn std::error::Error>> {
    let path = hmp_storage::data_dir().join("library.sqlite3");
    Ok(LibraryDb::open(&path)?)
}

/// track id → (source, source_key)：与引擎 `track_row` 同一规则
/// （`local:` 前缀 → local，其余 → qq），保证收藏/歌单与播放历史一致。
pub fn provider_of(id: &str) -> (&'static str, String) {
    if TrackProvider::from_id(id) == TrackProvider::Local {
        ("local", id.to_string())
    } else {
        ("qq", id.to_string())
    }
}
