//! 阶段 D 冒烟：歌单、专辑、歌手、排行榜、推荐（全部免登录接口）。
//!
//! 运行：`cargo run -p hmp-qqmusic-api --example browse_smoke`

use hmp_qqmusic_api::QqMusicClient;
use hmp_qqmusic_api::album::AlbumApi;
use hmp_qqmusic_api::recommend::RecommendApi;
use hmp_qqmusic_api::singer::{AreaType, GenreType, IndexType, SexType, SingerApi, TabType};
use hmp_qqmusic_api::songlist::SonglistApi;
use hmp_qqmusic_api::top::TopApi;

const JAY_MID: &str = "0025NhlN2yWrP4"; // 周杰伦

#[tokio::main]
async fn main() {
    let client = QqMusicClient::new();

    // 歌单详情
    let sl = SonglistApi::new(&client);
    let detail = sl
        .get_detail(8655927861, 0, 5, 1, false, true, true)
        .await
        .unwrap();
    println!(
        "歌单: {} (by {}, {} 首) 前3首: {}",
        detail.info.list.title,
        detail.info.creator.nick,
        detail.total,
        detail
            .songs
            .iter()
            .take(3)
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join(" / ")
    );

    // 专辑
    let album = AlbumApi::new(&client);
    let ad = album.get_detail("003RMaRI1iFoYd").await.unwrap();
    println!(
        "专辑: {} - {} (公司: {}, 歌手: {})",
        ad.album.album.name,
        ad.album.album.time_public,
        ad.company.name,
        ad.singers
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join("/")
    );
    let asong = album.get_song("1458791", 5, 1).await.unwrap();
    println!(
        "专辑歌曲: 共 {} 首, 首曲: {}",
        asong.total_num, asong.song_list[0].name
    );
    let new_albums = album.get_new_album(1, 5, 1).await.unwrap();
    println!(
        "新碟(内地): {} 张, 最新: {}",
        new_albums.total, new_albums.albums[0].album.name
    );

    // 歌手
    let singer = SingerApi::new(&client);
    let list = singer
        .get_singer_list(AreaType::All, SexType::All, GenreType::All)
        .await
        .unwrap();
    println!(
        "歌手列表: {} 位, 热门前3: {}",
        list.singerlist.len(),
        list.hotlist
            .iter()
            .take(3)
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join(" / ")
    );
    let index = singer
        .get_singer_list_index(
            AreaType::China,
            SexType::All,
            GenreType::All,
            IndexType::Letter(b'Z'),
            1,
            10,
        )
        .await
        .unwrap();
    println!(
        "Z 姓歌手: {} 位, 首个: {}",
        index.total, index.base.singerlist[0].name
    );
    let header = singer.get_info(JAY_MID).await.unwrap();
    println!(
        "歌手主页: {} (头像: {}...)",
        header.singer.name,
        &header.base_info.avatar[..header.base_info.avatar.len().min(50)]
    );
    let tab = singer
        .get_tab_detail(JAY_MID, TabType::Song, 1, 5)
        .await
        .unwrap();
    println!(
        "歌手歌曲 Tab: {} 首 (还有更多: {})",
        tab.song_tab.len(),
        tab.has_more > 0
    );
    let ss = singer.get_songs_list(JAY_MID, 5, 1).await.unwrap();
    println!(
        "歌手歌曲: 共 {} 首, 首曲: {}",
        ss.total_num, ss.song_list[0].name
    );
    let albums = singer.get_album_list(JAY_MID, 5, 1).await.unwrap();
    println!(
        "歌手专辑: {} 张, 最新: {}",
        albums.total, albums.album_list[0].album.name
    );
    let mvs = singer.get_mv_list(JAY_MID, 5, 1).await.unwrap();
    println!("歌手 MV: {} 个, 首个: {}", mvs.total, mvs.mv_list[0].title);
    let similar = singer.get_similar(JAY_MID, 5).await.unwrap();
    println!(
        "相似歌手: {}",
        similar
            .singerlist
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join(" / ")
    );
    // 实测 ex_singer/group_singer 会触发 10006，最小参数（仅 pic）可用
    let desc = singer
        .get_desc(&[JAY_MID.to_string()], false, false, false, true, false)
        .await
        .unwrap();
    println!(
        "歌手简介: {} (地区: {}, 生日: {})",
        desc.singer_list[0].basic_info.name,
        desc.singer_list[0].ex_info.area,
        desc.singer_list[0].ex_info.birthday
    );

    // 排行榜
    let top = TopApi::new(&client);
    let cats = top.get_category().await.unwrap();
    let first = &cats.group[0].toplist[0];
    println!(
        "排行榜分类: {} 组, 首个榜单: {} (id={})",
        cats.group.len(),
        first.name,
        first.id
    );
    let td = top.get_detail(first.id, 5, 1, true).await.unwrap();
    println!(
        "榜单详情: {} - 第1名: {}",
        td.info.name,
        td.songs[0]
            .singer
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join("/")
            + &format!(" {}", td.songs[0].name)
    );

    // 推荐
    let rec = RecommendApi::new(&client);
    let feed = rec.get_home_feed(1, 0, 0, &[]).await.unwrap();
    println!("首页推荐: {} 个楼层", feed.shelves.len());
    let radar = rec.get_radar_recommend(1).await.unwrap();
    println!(
        "雷达推荐: {} 首, 首曲: {} (还有更多: {})",
        radar.songs.len(),
        radar.songs[0].name,
        radar.has_more
    );
    let rsl = rec.get_recommend_songlist(1, 10).await.unwrap();
    println!(
        "推荐歌单: {} 个 (还有更多: {})",
        rsl.songlists.len(),
        rsl.has_more
    );
    let ns = rec.get_recommend_newsong(5).await.unwrap();
    println!(
        "推荐新歌: {} 首, 首曲: {}",
        ns.songs.len(),
        ns.songs[0].name
    );
}
