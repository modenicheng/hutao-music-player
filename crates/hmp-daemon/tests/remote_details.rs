//! 远端详情数据域集成测试（wiremock QQ API，零真实出网）——D2 内容动线。
//!
//! 锚定 content.rs 三个只读自由函数（`remote_playlist_detail` /
//! `remote_album_detail` / `remote_artist_detail`）的出网请求形态与投影：
//! 挂载匹配**以 api 层源码的请求构造字段为准照抄**（songlist.rs / album.rs /
//! singer.rs 的 `CgiRequest::new` module/method/param），不凭记忆写端点名；
//! 覆盖：
//! 1. `CgiGetDiss`：正常页投影（含 hasmore 窗口语义）与 -100006 not-found；
//! 2. `GetAlbumDetail` + `GetAlbumSongList`：basicInfo/company/singerList/
//!    songList[*].songInfo 解析；
//! 3. `GetHomepageHeader`（Android comm，`SingerMid` 参数）+ 歌手歌曲/专辑
//!    列表：singer_pic/total/albumList 投影。
//!
//! 同 reconcile_playlist_tracks.rs 范式（musicu 信封 + `req_0` 匹配）；
//! 本组函数无盘副作用，无需 XDG 隔离。

use hmp_daemon::content;
use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::config::ClientConfig;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// 解析请求体的 `req_0` 字段（模块/方法/参数匹配用）。
fn req0(req: &Request) -> serde_json::Value {
    serde_json::from_slice(&req.body).unwrap_or(serde_json::json!({}))["req_0"].clone()
}

/// musicu 信封：`req_0.data` 载荷。
fn envelope(data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "code": 0, "req_0": { "code": 0, "data": data } })
}

/// 曲目 songInfo（Song 模型 alias 形态：singerName/albumName/logo）。
fn song_info(mid: &str, name: &str, album_mid: &str, pmid: &str) -> serde_json::Value {
    serde_json::json!({
        "id": 1000,
        "mid": mid,
        "name": name,
        "type": 1,
        "singer": [{"singerName": "歌手甲", "singerMid": "smid"}],
        "album": {"albumName": "专辑甲", "albumMid": album_mid, "logo": pmid},
        "interval": 180,
    })
}

fn client_for(base_url: &str) -> QqMusicClient {
    QqMusicClient::with_config(ClientConfig {
        base_url: base_url.to_owned(),
        ..Default::default()
    })
}

// ————————————————————————————————————————————————
// CgiGetDiss（远端歌单详情）
// ————————————————————————————————————————————————

/// `CgiGetDiss` data 载荷（api 层键：dirinfo/songlist/total_song_num/hasmore）。
fn diss_data(code: i64, hasmore: i64, total: i64) -> serde_json::Value {
    serde_json::json!({
        "code": code,
        "dirinfo": {
            "dissid": 8522132521i64,
            "dissname": "华语经典",
            "picurl": "https://qpic.y.qq.com/music_cover/p.jpg",
            "listennum": 12345,
            "songnum": total,
            "creator": {"musicid": 7, "nick": "胡桃"},
        },
        "songlist": [
            song_info("m-1", "歌一", "am-1", "pmid-1"),
            song_info("m-2", "歌二", "am-2", ""),
        ],
        "songlist_size": 2,
        "total_song_num": total,
        "hasmore": hasmore,
    })
}

/// 挂载一个 disstid 匹配的 CgiGetDiss mock（api 层 param 键 `disstid`，
/// songlist.rs `CgiRequest::new` 构造）。
async fn mount_diss(server: &MockServer, disstid: i64, data: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(move |req: &Request| {
            req0(req)["module"] == serde_json::json!("music.srfDissInfo.DissInfo")
                && req0(req)["param"]["disstid"] == serde_json::json!(disstid)
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(data)))
        .mount(server)
        .await;
}

#[tokio::test]
async fn remote_playlist_detail_projects_window_and_fields() {
    let server = MockServer::start().await;
    mount_diss(&server, 8522132521, diss_data(0, 1, 268)).await;
    let client = client_for(&server.uri());

    let page = content::remote_playlist_detail(&client, 8522132521)
        .await
        .expect("detail ok");
    assert_eq!(page.id, 8522132521);
    assert_eq!(page.name, "华语经典");
    assert_eq!(page.creator, "胡桃");
    assert_eq!(page.picurl, "https://qpic.y.qq.com/music_cover/p.jpg");
    assert_eq!(page.listen_num, 12345);
    assert_eq!(page.total, 268);
    assert!(page.has_more, "hasmore=1 → 首窗之外还有更多");
    assert_eq!(page.tracks.len(), 2);
    // 曲目封面 pmid → T002 模板；空 pmid → 空串；行内链接带 mid
    assert_eq!(
        page.tracks[0].picurl,
        "https://y.gtimg.cn/music/photo_new/T002R300x300M000pmid-1.jpg"
    );
    assert_eq!(page.tracks[1].picurl, "");
    assert_eq!(page.tracks[0].album_mid, "am-1");
    assert_eq!(page.tracks[0].artist_mid, "smid");
}

#[tokio::test]
async fn remote_playlist_detail_maps_missing_playlist_to_err() {
    let server = MockServer::start().await;
    // 服务端「歌单不存在」= 内层 data.code=-100006（songlist.rs 实测记录）
    mount_diss(&server, 424242, diss_data(-100006, 0, 0)).await;
    let client = client_for(&server.uri());

    let err = content::remote_playlist_detail(&client, 424242)
        .await
        .expect_err("not-found 应报错");
    assert!(err.contains("not found"), "实际: {err}");
}

// ————————————————————————————————————————————————
// GetAlbumDetail + GetAlbumSongList（远端专辑详情）
// ————————————————————————————————————————————————

#[tokio::test]
async fn remote_album_detail_projects_header_and_songs() {
    let server = MockServer::start().await;
    // GetAlbumDetail（album.rs：module music.musichallAlbum.AlbumInfoServer，
    // MID 入参走 `albumMId` 键——注意大小写，api 层按此构造）
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &Request| {
            req0(req)["module"] == serde_json::json!("music.musichallAlbum.AlbumInfoServer")
                && req0(req)["method"] == serde_json::json!("GetAlbumDetail")
                && req0(req)["param"]["albumMId"] == serde_json::json!("amid-1")
        })
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                // desc/language/genre 位于 basicInfo 展开层（AlbumDetail 模型）
                "basicInfo": {
                    "albumID": 1458791,
                    "albumMid": "amid-1",
                    "albumName": "十一月的萧邦",
                    "logo": "pmid-x",
                    "publishDate": "2005-11-01",
                    "desc": "简介",
                    "language": "国语",
                    "genre": "流行",
                },
                "company": {"ID": 1, "name": "杰威尔音乐", "isShow": 1},
                "basicInfoExtra": {},
                "singer": {"singerList": [
                    {"singerMid": "smid", "singerName": "周杰伦"},
                ]},
            }))),
        )
        .mount(&server)
        .await;
    // GetAlbumSongList（module music.musichallAlbum.AlbumSongList，param
    // albumMid——注意与 GetAlbumDetail 的 `albumMId` 大小写不同，api 层
    // get_song 用 `albumMid`（album.rs:266）；歌曲位于 songList[*].songInfo）
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &Request| {
            req0(req)["module"] == serde_json::json!("music.musichallAlbum.AlbumSongList")
                && req0(req)["param"]["albumMid"] == serde_json::json!("amid-1")
        })
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "albumMid": "amid-1",
                "totalNum": 12,
                "songList": [
                    {"songInfo": song_info("m-1", "夜曲", "amid-1", "pmid-x")},
                    {"songInfo": song_info("m-2", "蓝色风暴", "amid-1", "pmid-x")},
                ],
            }))),
        )
        .mount(&server)
        .await;
    let client = client_for(&server.uri());

    let page = content::remote_album_detail(&client, "amid-1")
        .await
        .expect("album detail ok");
    assert_eq!(page.mid, "amid-1");
    assert_eq!(page.name, "十一月的萧邦");
    assert_eq!(page.singer, "周杰伦");
    assert_eq!(page.singer_mid, "smid");
    assert_eq!(page.company, "杰威尔音乐");
    assert_eq!(page.genre, "流行");
    assert_eq!(page.language, "国语");
    assert_eq!(page.desc, "简介");
    assert_eq!(page.time_public, "2005-11-01");
    assert_eq!(
        page.picurl,
        "https://y.gtimg.cn/music/photo_new/T002R300x300M000pmid-x.jpg"
    );
    assert_eq!(page.total, 12);
    assert!(page.has_more, "total 12 > 返回 2 → has_more（规则②）");
    assert_eq!(page.tracks.len(), 2);
}

#[tokio::test]
async fn remote_album_detail_rejects_empty_mid() {
    let server = MockServer::start().await;
    let client = client_for(&server.uri());
    // 空 mid 不出网（无 mock 挂载，若出网会连接失败/超时——这里断言直接 Err）
    let err = content::remote_album_detail(&client, "")
        .await
        .expect_err("空 mid 应拒绝");
    assert!(err.contains("empty"));
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        0
    );
}

// ————————————————————————————————————————————————
// GetHomepageHeader + GetSingerSongList + GetAlbumList（远端歌手详情）
// ————————————————————————————————————————————————

#[tokio::test]
async fn remote_artist_detail_projects_photo_songs_and_albums() {
    let server = MockServer::start().await;
    // GetHomepageHeader（singer.rs：module music.UnifiedHomepage.
    // UnifiedHomepageSrv，param `SingerMid`——Android comm，键大写开头；
    // 歌手信息位于 Info.Singer，由 extract_homepage_info 提取）
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &Request| {
            req0(req)["module"] == serde_json::json!("music.UnifiedHomepage.UnifiedHomepageSrv")
                && req0(req)["param"]["SingerMid"] == serde_json::json!("smid")
        })
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "Status": 0,
                "Info": {
                    // 键形照抄 fixture homepage_header.json：`Info.Singer.Name`
                    //（非 SingerName；HomepageSinger.name 别名 Name/singerName）
                    "Singer": {
                        "SingerID": 4558,
                        "SingerMid": "smid",
                        "Name": "周杰伦",
                        "SingerPic": "https://y.gtimg.cn/music/photo_new/T001R150x150M000x_11.jpg",
                    },
                    "BaseInfo": {"Name": "周杰伦", "Avatar": "https://y.gtimg.cn/a.jpg"},
                },
            }))),
        )
        .mount(&server)
        .await;
    // GetSingerSongList（module musichall.song_list_server，param
    // singerMid/begin/number；songList[*].songInfo）
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &Request| {
            req0(req)["module"] == serde_json::json!("musichall.song_list_server")
                && req0(req)["param"]["singerMid"] == serde_json::json!("smid")
        })
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "singerMid": "smid",
                "totalNum": 1012,
                "songList": [{"songInfo": song_info("m-1", "青花瓷", "am-1", "pmid-1")}],
            }))),
        )
        .mount(&server)
        .await;
    // GetAlbumList（module music.musichallAlbum.AlbumListServer，param
    // singerMid/begin/number；专辑位于 albumList[*]（AlbumBrief 展开））
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(|req: &Request| {
            req0(req)["module"] == serde_json::json!("music.musichallAlbum.AlbumListServer")
                && req0(req)["param"]["singerMid"] == serde_json::json!("smid")
        })
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
                "singerMid": "smid",
                "total": 139,
                "albumList": [
                    {
                        "albumMid": "a-1",
                        "albumName": "范特西",
                        "logo": "pA",
                        "publishDate": "2001-09-14",
                        "totalNum": 10,
                        "singerName": "周杰伦",
                    },
                ],
            }))),
        )
        .mount(&server)
        .await;
    let client = client_for(&server.uri());

    let page = content::remote_artist_detail(&client, "smid")
        .await
        .expect("artist detail ok");
    assert_eq!(page.mid, "smid");
    assert_eq!(page.name, "周杰伦");
    // 歌手照片 = Info.Singer.SingerPic（§20/§23 边界闭合的数据源）
    assert_eq!(
        page.photo_url,
        "https://y.gtimg.cn/music/photo_new/T001R150x150M000x_11.jpg"
    );
    assert_eq!(page.song_total, 1012);
    assert_eq!(page.album_total, 139);
    assert_eq!(page.tracks.len(), 1);
    assert_eq!(page.desc, "");
    assert_eq!(page.albums.len(), 1);
    assert_eq!(page.albums[0].mid, "a-1");
    assert_eq!(page.albums[0].year_text, "2001");
    assert_eq!(page.albums[0].track_count, 10);
    assert_eq!(
        page.albums[0].picurl,
        "https://y.gtimg.cn/music/photo_new/T002R300x300M000pA.jpg"
    );
}

#[tokio::test]
async fn remote_artist_detail_rejects_empty_mid() {
    let server = MockServer::start().await;
    let client = client_for(&server.uri());
    let err = content::remote_artist_detail(&client, "")
        .await
        .expect_err("空 mid 应拒绝");
    assert!(err.contains("empty"));
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        0
    );
}
