//! 阶段 C 冒烟：歌曲详情、播放 URL（试听）、歌词解密。
//!
//! 运行：`cargo run -p hmp-qqmusic-api --example song_smoke`

use hmp_qqmusic_api::QqMusicClient;
use hmp_qqmusic_api::lyric::LyricApi;
use hmp_qqmusic_api::song::{SongApi, SongFileInfo, SongFileType};

#[tokio::main]
async fn main() {
    let client = QqMusicClient::new();
    let song_api = SongApi::new(&client);

    // 歌曲详情（"开始懂了" by 孙燕姿）
    let detail = song_api.get_detail("186016").await.expect("get detail");
    println!(
        "detail: {} - {} (album: {}, media_mid: {})",
        detail.track.name,
        detail
            .track
            .singer
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join("/"),
        detail.track.album.name,
        detail.track.file.media_mid,
    );

    // 播放 URL（试听类型，免登录）
    let urls = song_api
        .get_song_urls(
            &[SongFileInfo {
                mid: detail.track.mid.clone(),
                file_type: Some(SongFileType::TRY),
                song_type: 0,
                media_mid: Some(detail.track.file.media_mid.clone()),
            }],
            SongFileType::TRY,
            None,
        )
        .await
        .expect("get song urls");
    for (i, url) in urls.build_urls().iter().enumerate() {
        println!("url[{i}]: {}", url.as_deref().unwrap_or("<no purl>"));
    }

    // 歌词（自动解密 QRC）
    let lyric_api = LyricApi::new(&client);
    let lyric = lyric_api
        .get_lyric("186016", 1, false, true, false, false)
        .await
        .expect("get lyric");
    println!(
        "lyric: {} chars, first line: {}",
        lyric.lyric.len(),
        lyric.lyric.lines().next().unwrap_or("")
    );
}
