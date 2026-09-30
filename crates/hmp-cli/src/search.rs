//! `hmp search`：搜索歌曲并输出结果。

use hmp_qqmusic_api::QqMusicClient;

use super::output;

/// 搜索并打印歌曲列表（`<index>: <歌曲名> - <歌手> [<songmid>]`）。
pub async fn run(keyword: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let client = QqMusicClient::new();
    let result = client.quick_search(keyword).await?;

    if json {
        // QuickSearch 非 Serialize（api crate DTO），此处构造稳定投影。
        let songs: Vec<serde_json::Value> = result
            .songs
            .iter()
            .map(|s| serde_json::json!({"mid": s.mid, "name": s.name, "singer": s.singer}))
            .collect();
        let albums: Vec<serde_json::Value> = result
            .albums
            .iter()
            .map(|a| serde_json::json!({"mid": a.mid, "name": a.name, "singer": a.singer}))
            .collect();
        let singers: Vec<serde_json::Value> = result
            .singers
            .iter()
            .map(|s| serde_json::json!({"mid": s.mid, "name": s.name}))
            .collect();
        return output::print(&serde_json::json!({
            "keyword": keyword,
            "songs": songs,
            "albums": albums,
            "singers": singers,
        }));
    }

    if result.songs.is_empty() {
        println!("No results for \"{keyword}\"");
        return Ok(());
    }
    println!("{} result(s) for \"{keyword}\":", result.songs.len());
    for (i, song) in result.songs.iter().enumerate() {
        println!(
            "{:>3}. {} - {}  [{}]",
            i + 1,
            song.name,
            song.singer,
            song.mid
        );
    }
    println!();
    println!("Play: hmp play <songmid>");
    Ok(())
}
