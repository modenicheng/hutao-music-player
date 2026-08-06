//! 免登录搜索冒烟测试（阶段 A 验收）：对官方端点执行 quick_search。
//! 用法: cargo run -p hmp-qqmusic-api --example smoke_search -- "周杰伦"

use hmp_qqmusic_api::QqMusicClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let keyword = std::env::args().nth(1).unwrap_or_else(|| "周杰伦".into());
    let client = QqMusicClient::new();
    let quick = client.quick_search(&keyword).await?;
    println!("query: {keyword}");
    println!("songs: {}", quick.songs.len());
    for s in quick.songs.iter().take(5) {
        println!("  {:<12} {:<8} mid={}", s.name, s.singer, s.mid);
    }
    println!(
        "albums: {}  singers: {}",
        quick.albums.len(),
        quick.singers.len()
    );
    Ok(())
}
