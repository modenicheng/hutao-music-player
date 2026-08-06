//! `hmp search`：搜索歌曲并输出结果。

use hmp_qqmusic_api::QqMusicClient;

/// 搜索并打印歌曲列表（`<index>: <歌曲名> - <歌手> [<songmid>]`）。
pub async fn run(keyword: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = QqMusicClient::new();
    let result = client.quick_search(keyword).await?;

    if result.songs.is_empty() {
        println!("没有找到与「{keyword}」相关的歌曲");
        return Ok(());
    }
    println!("搜索「{keyword}」共 {} 个结果:", result.songs.len());
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
    println!("播放: hmp play <songmid>");
    Ok(())
}
