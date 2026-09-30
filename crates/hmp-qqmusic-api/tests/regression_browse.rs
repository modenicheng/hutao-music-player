//! browse 域（album/top/recommend/comment）回归测试（wiremock 离线）。
//!
//! 每个用例锚定一个 2026-09-29 现场实测结论（探针：
//! `examples/live_browse.rs`），防止移植回退：
//! - `AddComment` 响应的新评论 ID 键为**内层 `data.AddedCmId`**（无
//!   `commentId`/`CmId` 键；此前别名缺失导致 `comment_id` 恒为空串，
//!   daemon 无法回传可删除的评论 ID）；
//! - `DelComment` 响应的判定键为**内层 `data.Subcode`**（注意大小写，与
//!   AddComment 的 `SubCode` 不同；此前在子响应顶层找 `SubCode` 导致
//!   恒为 `false`）；键缺省按上游 `data.get("SubCode", 0)` 语义视为 0；
//! - `PlaylistSquare/GetRecommendFeed` 的歌单封面/创建者位于
//!   `List[*].Playlist.basic` 下（`Playlist` 直下无 `cover`/`creator` 键；
//!   此前提取层级错误导致 `picurl`/`creator_nick` 恒为空）；
//! - `AlbumFavWrite` 的 `result`/`v_failedAlbumId` 在内层 data；
//! - `Toplist/GetDetail` 的 `withTags` 为 JSON 布尔（与需 0/1 整数的
//!   `GetSingerDetail` 不同），`withTags=false`（tag=false）时不携带该键。

use hmp_qqmusic_api::album::AlbumApi;
use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::comment::CommentApi;
use hmp_qqmusic_api::config::ClientConfig;
use hmp_qqmusic_api::credential::{Credential, LoginType};
use hmp_qqmusic_api::error::QqMusicError;
use hmp_qqmusic_api::pagination::Page;
use hmp_qqmusic_api::recommend::RecommendApi;
use hmp_qqmusic_api::top::TopApi;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_for(base_url: &str) -> QqMusicClient {
    let config = ClientConfig {
        base_url: base_url.to_owned(),
        ..Default::default()
    };
    QqMusicClient::with_config(config)
}

fn cred() -> Credential {
    Credential {
        uin: "939861972".into(),
        music_id: "939861972".into(),
        music_key: "test-key".into(),
        login_type: LoginType::Qq,
        encrypt_uin: "NKoqNeC5NKSA".into(),
        ..Default::default()
    }
}

/// 校验 `req_0` 的 module/method/param 的通用 matcher 工厂。
fn req_matches(
    module: &'static str,
    method: &'static str,
    check: impl Fn(&serde_json::Value) -> bool + Send + Sync + 'static,
) -> impl wiremock::Match {
    move |req: &wiremock::Request| {
        let body: serde_json::Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let req_0 = &body["req_0"];
        req_0["module"] == json!(module)
            && req_0["method"] == json!(method)
            && check(&req_0["param"])
    }
}

fn ok_sub(data: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"code": 0, "req_0": {"code": 0, "data": data}}))
}

// ---------- album ----------

/// GetAlbumDetail：MID 入参走 `albumMId`；`basicInfo`/`company`/
/// `singer.singerList` 解析（2026-09-29 实测键形）。
#[tokio::test]
async fn album_detail_mid_param_and_basic_info_parse() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musichallAlbum.AlbumInfoServer",
            "GetAlbumDetail",
            |param| param["albumMId"] == json!("003RMaRI1iFoYd"),
        ))
        .respond_with(ok_sub(json!({
            "basicInfo": {
                "albumID": 1458791,
                "albumMid": "003RMaRI1iFoYd",
                "albumName": "周杰伦的床边故事",
                "publishDate": "2016-06-24",
                "genre": "流行",
                "language": "国语",
                "albumType": "录音室专辑",
                "wikiurl": "https://wiki.example"
            },
            "company": {"ID": 101, "name": "杰威尔音乐有限公司", "isShow": 1, "brief": ""},
            "singer": {"singerList": [
                {"singerID": 4558, "singerMid": "0025NhlN2yWrP4", "singerName": "周杰伦"}
            ]}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = AlbumApi::new(&client);
    let resp = api.get_detail("003RMaRI1iFoYd").await.unwrap();
    assert_eq!(resp.album.album.id, 1458791);
    assert_eq!(resp.album.album.name, "周杰伦的床边故事");
    assert_eq!(resp.album.album.time_public, "2016-06-24");
    assert_eq!(resp.album.album_type, "录音室专辑");
    assert_eq!(resp.company.name, "杰威尔音乐有限公司");
    assert_eq!(resp.company.id, 101);
    assert_eq!(resp.singers.len(), 1);
    assert_eq!(resp.singers[0].name, "周杰伦");
}

/// GetAlbumDetail：数字 ID 入参走 `albumId`。
#[tokio::test]
async fn album_detail_numeric_param_uses_album_id_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musichallAlbum.AlbumInfoServer",
            "GetAlbumDetail",
            |param| param["albumId"] == json!(1458791),
        ))
        .respond_with(ok_sub(json!({
            "basicInfo": {"albumID": 1458791, "albumMid": "003RMaRI1iFoYd", "albumName": "周杰伦的床边故事"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = AlbumApi::new(&client);
    let resp = api.get_detail("1458791").await.unwrap();
    assert_eq!(resp.album.album.id, 1458791);
}

/// GetAlbumSongList：分页参数 `begin`/`num`；歌曲取自
/// `songList[*].songInfo`，`totalNum` 为专辑曲数。
#[tokio::test]
async fn album_song_list_pagination_and_song_info_parse() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musichallAlbum.AlbumSongList",
            "GetAlbumSongList",
            |param| {
                param["albumMid"] == json!("003RMaRI1iFoYd")
                    && param["begin"] == json!(5)
                    && param["num"] == json!(5)
            },
        ))
        .respond_with(ok_sub(json!({
            "albumMid": "003RMaRI1iFoYd",
            "totalNum": 10,
            "songList": [
                {"songInfo": {"id": 107192080, "mid": "04CHmZ7C1s6OGR", "name": "说好不哭"}},
                {"songInfo": {"id": 107192081, "mid": "04CHmZ7C1s6OH1", "name": "不该"}}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = AlbumApi::new(&client);
    let resp = api.get_song("003RMaRI1iFoYd", Page::new(2, 5)).await.unwrap();
    assert_eq!(resp.total_num, 10);
    assert_eq!(resp.song_list.len(), 2);
    assert_eq!(resp.song_list[0].id, 107192080);
    assert_eq!(resp.song_list[0].name, "说好不哭");
}

/// get_new_album_info：参数 `area`/`num`/`start`；响应 `total`/`albums`。
#[tokio::test]
async fn album_new_album_params_and_parse() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "newalbum.NewAlbumServer",
            "get_new_album_info",
            |param| {
                param["area"] == json!(6)
                    && param["num"] == json!(5)
                    && param["start"] == json!(0)
            },
        ))
        .respond_with(ok_sub(json!({
            "total": 2381,
            "albums": [
                {"albumID": 50446765, "albumMid": "002eFUFm2ABC7z", "albumName": "Lagal",
                 "singers": [{"singerID": 1, "singerMid": "m1", "singerName": "A"}],
                 "release_time": "2026-09-20", "type": 1, "area": 6, "genre": 0, "language": 0}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = AlbumApi::new(&client);
    let resp = api.get_new_album(6, Page::new(1, 5)).await.unwrap();
    assert_eq!(resp.total, 2381);
    assert_eq!(resp.albums.len(), 1);
    assert_eq!(resp.albums[0].album.id, 50446765);
    assert_eq!(resp.albums[0].release_time, "2026-09-20");
    assert_eq!(resp.albums[0].singers[0].name, "A");
}

/// 回归锚点（2026-09-29 实测）：AlbumFavWrite 的 `result`/`v_failedAlbumId`
/// 在**内层 data**；fav 成功判定 result=0 且无失败项。
#[tokio::test]
async fn album_fav_unwraps_inner_data_and_judges_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.AlbumFavWrite",
            "FavAlbum",
            |param| param["v_albumId"] == json!([50446765]),
        ))
        .respond_with(ok_sub(json!({
            "imported_album_cnt": 0,
            "order_album_cnt": 0,
            "reason": "",
            "result": 0,
            "v_failedAlbumId": [],
            "v_failedAlbumMid": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = AlbumApi::new(&client);
    let resp = api.fav_album(&[50446765], &cred()).await.unwrap();
    assert!(resp.success());
    assert_eq!(resp.result, 0);
    assert!(resp.failed_album_id.is_empty());
}

/// CancelFavAlbum 同形（净零闭环实测：取消后 result=0）。
#[tokio::test]
async fn album_del_fav_unwraps_inner_data() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.AlbumFavWrite",
            "CancelFavAlbum",
            |param| param["v_albumId"] == json!([50446765]),
        ))
        .respond_with(ok_sub(json!({
            "imported_album_cnt": 0,
            "order_album_cnt": 0,
            "reason": "",
            "result": 0,
            "v_failedAlbumId": [],
            "v_failedAlbumMid": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = AlbumApi::new(&client);
    let resp = api.del_fav_album(&[50446765], &cred()).await.unwrap();
    assert!(resp.success());
}

/// fav/del 需登录：缺有效凭证 → AuthenticationRequired（不发请求）。
#[tokio::test]
async fn album_fav_requires_credential() {
    let server = MockServer::start().await;
    let client = client_for(&server.uri());
    let api = AlbumApi::new(&client);
    assert!(matches!(
        api.fav_album(&[1], &Credential::default()).await,
        Err(QqMusicError::AuthenticationRequired)
    ));
    assert!(matches!(
        api.del_fav_album(&[1], &Credential::default()).await,
        Err(QqMusicError::AuthenticationRequired)
    ));
}

// ---------- top ----------

/// GetAll：空参数；`group[].groupId/groupName/toplist` 解析。
#[tokio::test]
async fn top_category_parses_group_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicToplist.Toplist",
            "GetAll",
            |param| param.as_object().is_some_and(|m| m.is_empty()),
        ))
        .respond_with(ok_sub(json!({
            "group": [
                {"groupId": 1, "groupName": "巅峰榜", "toplist": [
                    {"topId": 62, "title": "飙升榜", "titleDetail": "QQ音乐飙升榜",
                     "titleSub": "每天更新", "listenNum": 2089405, "totalNum": 100,
                     "updateTime": "2026-09-29", "frontPicUrl": "https://img.example/f",
                     "song": [
                        {"rank": 1, "rankType": 1, "rankValue": "1", "songId": 107192080,
                         "title": "说好不哭", "singerName": "周杰伦", "singerMid": "0025NhlN2yWrP4",
                         "albumMid": "003RMaRI1iFoYd", "cover": "https://img.example/c", "mvid": 0}
                     ]}
                ]}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = TopApi::new(&client);
    let resp = api.get_category().await.unwrap();
    assert_eq!(resp.group.len(), 1);
    let g = &resp.group[0];
    assert_eq!(g.id, 1);
    assert_eq!(g.name, "巅峰榜");
    let t = &g.toplist[0];
    assert_eq!(t.id, 62);
    assert_eq!(t.name, "飙升榜");
    assert_eq!(t.listen_num, 2089405);
    assert_eq!(t.songs.len(), 1);
    assert_eq!(t.songs[0].name, "说好不哭");
    assert_eq!(t.songs[0].singer_name, "周杰伦");
}

/// GetDetail：`withTags` 为 JSON 布尔（tag=true 时携带）；榜单信息取自
/// 内层 `data` 键（别名 `data` → `info`），歌曲取自 `songInfoList`。
#[tokio::test]
async fn top_detail_sends_with_tags_bool_and_parses() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicToplist.Toplist",
            "GetDetail",
            |param| {
                param["topId"] == json!(62)
                    && param["offset"] == json!(0)
                    && param["num"] == json!(5)
                    && param["withTags"] == json!(true)
            },
        ))
        .respond_with(ok_sub(json!({
            "data": {
                "topId": 62, "title": "飙升榜", "titleSub": "每天更新",
                "updateTime": "2026-09-29", "listenNum": 2089405, "totalNum": 100
            },
            "songInfoList": [
                {"id": 107192080, "mid": "04CHmZ7C1s6OGR", "name": "说好不哭",
                 "singer": [{"singerMid": "0025NhlN2yWrP4", "singerName": "周杰伦"}]}
            ],
            "songTagInfoList": null,
            "extInfoList": [],
            "indexInfoList": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = TopApi::new(&client);
    let resp = api.get_detail(62, Page::new(1, 5), true).await.unwrap();
    assert_eq!(resp.info.id, 62);
    assert_eq!(resp.info.name, "飙升榜");
    assert_eq!(resp.info.total_num, 100);
    assert_eq!(resp.songs.len(), 1);
    assert_eq!(resp.songs[0].mid, "04CHmZ7C1s6OGR");
    // null 列表按上游 NoneToEmptyList 语义归零
    assert!(resp.song_tags.is_empty());
    assert!(resp.index_info_list.is_empty());
}

/// tag=false 时不携带 `withTags` 键（上游 `preserve_bool=tag` 语义）。
#[tokio::test]
async fn top_detail_without_tag_omits_with_tags() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicToplist.Toplist",
            "GetDetail",
            |param| {
                param["topId"] == json!(62)
                    && param.get("withTags").is_none()
            },
        ))
        .respond_with(ok_sub(json!({
            "data": {"topId": 62, "title": "飙升榜", "totalNum": 100},
            "songInfoList": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = TopApi::new(&client);
    let resp = api.get_detail(62, Page::new(2, 5), false).await.unwrap();
    assert_eq!(resp.info.id, 62);
    assert!(resp.songs.is_empty());
}

// ---------- recommend ----------

/// GetRecommendFeed（歌单广场）：封面/创建者取自 `List[*].Playlist.basic`
/// 下（回归锚点 2026-09-29：此前误读 `Playlist.cover` 层级导致恒为空）。
#[tokio::test]
async fn recommend_songlist_extracts_basic_cover_and_creator() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.playlist.PlaylistSquare",
            "GetRecommendFeed",
            |param| param["From"] == json!(0) && param["Size"] == json!(5),
        ))
        .respond_with(ok_sub(json!({
            "Msg": "",
            "List": [
                {"Playlist": {
                    "basic": {
                        "tid": 9282300617i64,
                        "dirid": 0,
                        "title": "华语精选",
                        "desc": "",
                        "song_cnt": 30,
                        "play_cnt": 1234567,
                        "cover": {
                            "id": 0,
                            "mid": "",
                            "small_url": "http://qpic.example/300",
                            "default_url": "http://qpic.example/default",
                            "big_url": "http://qpic.example/big"
                        },
                        "creator": {
                            "uin": "511074779",
                            "encrypt_uin": "7K65oeSP7iSq",
                            "nick": "左耳先森",
                            "avatar": "https://pic.example/avatar"
                        }
                    },
                    "content": {},
                    "bHit": 0,
                    "diy": {}
                }}
            ],
            "HasMore": true,
            "FromLimit": 5
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = RecommendApi::new(&client);
    let resp = api.get_recommend_songlist(Page::new(1, 5)).await.unwrap();
    assert!(resp.has_more);
    assert_eq!(resp.from_limit, 5);
    assert_eq!(resp.songlists.len(), 1);
    let sl = &resp.songlists[0];
    assert_eq!(sl.id, 9282300617);
    assert_eq!(sl.title, "华语精选");
    assert_eq!(sl.songnum, 30);
    assert_eq!(sl.listennum, 1234567);
    // 回归锚点：封面/创建者必须从 basic 下提取
    assert_eq!(sl.picurl, "http://qpic.example/default");
    assert_eq!(sl.creator_nick, "左耳先森");
}

/// 兼容回退：`Playlist` 直下携带 `cover`/`creator` 的历史形态仍可解析。
#[tokio::test]
async fn recommend_songlist_playlist_level_cover_fallback() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok_sub(json!({
            "List": [
                {"Playlist": {
                    "basic": {"tid": 42, "title": "回退歌单"},
                    "cover": {"default_url": "https://img.example/fallback"},
                    "creator": {"nick": "回退用户"}
                }}
            ],
            "HasMore": false,
            "FromLimit": 0
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = RecommendApi::new(&client);
    let resp = api.get_recommend_songlist(Page::new(1, 1)).await.unwrap();
    assert_eq!(resp.songlists[0].picurl, "https://img.example/fallback");
    assert_eq!(resp.songlists[0].creator_nick, "回退用户");
}

/// 首页 Feed：`v_shelf`/`v_niche`/`v_card` 键形解析。
#[tokio::test]
async fn home_feed_parses_v_shelf_and_v_niche() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.recommend.RecommendFeed",
            "get_recommend_feed",
            |param| {
                param["direction"] == json!(0)
                    && param["page"] == json!(1)
                    && param["s_num"] == json!(0)
                    && param["v_cache"] == json!([])
            },
        ))
        .respond_with(ok_sub(json!({
            "retcode": 0,
            "msg": "",
            "prompt": "",
            "d_num": 1,
            "load_mark": 0,
            "v_shelf": [
                {"id": 100, "title_template": "1", "title_content": "电台",
                 "more": {}, "v_niche": [
                    {"id": 1, "title_template": "1", "title_content": "私人雷达",
                     "v_card": [{"card_name": "radar"}]}
                 ]}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = RecommendApi::new(&client);
    let resp = api.get_home_feed(1, 0, 0, &[]).await.unwrap();
    assert_eq!(resp.retcode, 0);
    assert_eq!(resp.shelves.len(), 1);
    assert_eq!(resp.shelves[0].niches.len(), 1);
    assert_eq!(resp.shelves[0].niches[0].cards.len(), 1);
}

/// 回归锚点（2026-09-29 实测）：猜你喜欢数据键 `tracks`，需登录态。
#[tokio::test]
async fn guess_recommend_parses_tracks_and_requires_credential() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.radioProxy.MbTrackRadioSvr",
            "get_radio_track",
            |param| {
                param["id"] == json!(99)
                    && param["num"] == json!(5)
                    && param["song_ids"] == json!([])
            },
        ))
        .respond_with(ok_sub(json!({
            "id": 99,
            "name": "猜你喜欢",
            "tracks": [
                {"id": 107192080, "mid": "04CHmZ7C1s6OGR", "name": "说好不哭", "type": 0}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = RecommendApi::new(&client);
    let resp = api.get_guess_recommend(&cred()).await.unwrap();
    assert_eq!(resp.songs.len(), 1);
    assert_eq!(resp.songs[0].mid, "04CHmZ7C1s6OGR");
    // 免登录凭证缺 valid key → 客户端直接拒绝（require_login）
    assert!(matches!(
        api.get_guess_recommend(&Credential::default()).await,
        Err(QqMusicError::AuthenticationRequired)
    ));
}

/// 雷达推荐：`VecSongs[*].Track` + `RecommendSongIds`/`HasMore`。
#[tokio::test]
async fn radar_recommend_parses_vec_songs() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.recommend.TrackRelationServer",
            "GetRadarSong",
            |param| {
                param["Page"] == json!(1)
                    && param["ReqType"] == json!(0)
                    && param["FavSongs"] == json!([])
            },
        ))
        .respond_with(ok_sub(json!({
            "VecSongs": [
                {"Track": {"id": 1, "mid": "m1", "name": "A", "type": 0}},
                {"Track": {"id": 2, "mid": "m2", "name": "B", "type": 0}}
            ],
            "RecommendSongIds": [1, 2],
            "BaseSongIds": [9],
            "HasMore": true,
            "Toast": "根据您的收藏发现了2首歌曲",
            "TimeStamp": 1780000000,
            "VideoCards": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = RecommendApi::new(&client);
    let resp = api.get_radar_recommend(1).await.unwrap();
    assert_eq!(resp.songs.len(), 2);
    assert_eq!(resp.songs[1].name, "B");
    assert_eq!(resp.recommend_song_ids, vec![1, 2]);
    assert_eq!(resp.base_song_ids, vec![9]);
    assert!(resp.has_more);
    assert_eq!(resp.toast, "根据您的收藏发现了2首歌曲");
}

/// 推荐新歌：`songlist`/`songTagInfoList`/`type` 键形解析。
#[tokio::test]
async fn recommend_newsong_parses_songlist_and_tags() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "newsong.NewSongServer",
            "get_new_song_info",
            |param| param["type"] == json!(5),
        ))
        .respond_with(ok_sub(json!({
            "lanlist": [{"lan": "最新"}],
            "lan": "最新",
            "songlist": [
                {"id": 1, "mid": "m1", "name": "新歌A", "type": 0}
            ],
            "ret_msg": "",
            "type": 5,
            "songTagInfoList": [
                {"id": 1, "tagid": 7, "tag": "内地", "link": "", "from_type": 0}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = RecommendApi::new(&client);
    let resp = api.get_recommend_newsong(5).await.unwrap();
    assert_eq!(resp.type_, 5);
    assert_eq!(resp.lan, "最新");
    assert_eq!(resp.songs.len(), 1);
    assert_eq!(resp.song_tags.len(), 1);
    assert_eq!(resp.song_tags[0].tag, "内地");
}

// ---------- comment ----------

/// GetCmCount：双层 `request` 包裹（biz_id 为字符串）；取 `data.response.count`。
#[tokio::test]
async fn comment_count_sends_wrapped_request_and_extracts_count() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.globalComment.CommentCountSrv",
            "GetCmCount",
            |param| {
                param["request"]["biz_id"] == json!("107192080")
                    && param["request"]["biz_type"] == json!(1)
                    && param["request"]["biz_sub_type"] == json!(2)
            },
        ))
        .respond_with(ok_sub(json!({
            "response": {"biz_id": "107192080", "biz_type": 1, "count": 38965}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = CommentApi::new(&client);
    let n = api.get_comment_count(107192080).await.unwrap();
    assert_eq!(n, 38965);
}

/// 三类评论列表：参数键形（`BizId` 字符串、`PageNum = page-1`、`PageSize`、
/// 各自特有键）与 `CommentList.Comments[]`（CmId/SeqNo/Nick/Content/
/// PraiseNum/PubTime/ReplyCnt）解析。
#[tokio::test]
async fn comment_lists_send_wire_params_and_parse_comments() {
    let server = MockServer::start().await;
    let cases: [(&str, &str, i64); 3] = [
        ("GetHotCommentList", "HotType", 1),
        ("GetNewCommentList", "SelfSeeEnable", 1),
        ("GetRecCommentList", "Flag", 1),
    ];
    for (method_name, marker_key, marker_value) in cases {
        Mock::given(method("POST"))
            .and(path("/cgi-bin/musicu.fcg"))
            .and(req_matches(
                "music.globalComment.CommentRead",
                method_name,
                move |param| {
                    param["BizId"] == json!("107192080")
                        && param["BizType"] == json!(1)
                        && param["BizSubType"] == json!(2)
                        && param["PageNum"] == json!(1)
                        && param["PageSize"] == json!(20)
                        && param[marker_key] == json!(marker_value)
                },
            ))
            .respond_with(ok_sub(json!({
                "CommentList": {
                    "Comments": [
                        {
                            "CmId": "1!ABC",
                            "SeqNo": "1628467045072956929",
                            "Nick": "胡桃",
                            "Content": "好听",
                            "PraiseNum": 42,
                            "PubTime": 1700000000,
                            "ReplyCnt": 3,
                            "Avatar": "https://img.example/a",
                            "EncryptUin": "NKoqNeC5NKSA"
                        }
                    ],
                    "HasMore": 1,
                    "Total": 100
                },
                "TotalCmNum": 38965
            })))
            .expect(1)
            .mount(&server)
            .await;
    }

    let client = client_for(&server.uri());
    let api = CommentApi::new(&client);
    for list in [
        api.get_hot_comments(107192080, Page::new(2, 20)).await.unwrap(),
        api.get_new_comments(107192080, Page::new(2, 20)).await.unwrap(),
        api.get_recommend_comments(107192080, Page::new(2, 20)).await.unwrap(),
    ] {
        let list = list.comment_list.map(|l| l.comments).unwrap_or_default();
        assert_eq!(list.len(), 1);
        let c = &list[0];
        assert_eq!(c.cm_id, "1!ABC");
        assert_eq!(c.seq_no, "1628467045072956929");
        assert_eq!(c.nickname, "胡桃");
        assert_eq!(c.content, "好听");
        assert_eq!(c.like_count, 42);
        assert_eq!(c.time, 1700000000);
        assert_eq!(c.reply_count, 3);
    }
}

/// 回归锚点（2026-09-29 修复）：AddComment 响应新评论 ID 键为内层
/// `AddedCmId`；`SubCode`/`Msg`/`ParentCmId`/`Floor.Num`/`VerifyUrl` 均解析。
#[tokio::test]
async fn add_comment_parses_added_cm_id_and_reply_param() {
    let server = MockServer::start().await;
    // 直发：Content/BizType/BizId/BizSubType，无 RepliedCmId
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.globalComment.CommentWriteServer",
            "AddComment",
            |param| {
                param["Content"] == json!("HMP API 测试评论，即将删除")
                    && param["BizId"] == json!("107192080")
                    && param["BizType"] == json!(1)
                    && param["BizSubType"] == json!(2)
                    && param.get("RepliedCmId").is_none()
            },
        ))
        .respond_with(ok_sub(json!({
            "AddedCmId": "1!mJJtUODvWTcDuoivU5x90L8nK3NapK15NE4naAVRaDE9cE2zOqsUVw99W-TvWUXQ",
            "SubCode": 0,
            "Msg": "发表成功",
            "ParentCmId": "",
            "VerifyUrl": "",
            "Floor": {"Num": 41721, "ShowPopup": 0}
        })))
        .expect(1)
        .mount(&server)
        .await;
    // 回复：带 RepliedCmId
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.globalComment.CommentWriteServer",
            "AddComment",
            |param| param["RepliedCmId"] == json!("1!PARENT"),
        ))
        .respond_with(ok_sub(json!({
            "AddedCmId": "1!CHILD",
            "SubCode": 0,
            "Msg": "发表成功",
            "ParentCmId": "1!PARENT",
            "Floor": {"Num": 41722}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = CommentApi::new(&client);
    let resp = api
        .add_comment(107192080, "HMP API 测试评论，即将删除", None, &cred())
        .await
        .unwrap();
    // 回归锚点：此前别名缺失导致 comment_id 恒为空串
    assert_eq!(
        resp.comment_id,
        "1!mJJtUODvWTcDuoivU5x90L8nK3NapK15NE4naAVRaDE9cE2zOqsUVw99W-TvWUXQ"
    );
    assert_eq!(resp.subcode, 0);
    assert_eq!(resp.msg, "发表成功");
    assert_eq!(resp.floor, 41721);
    assert_eq!(resp.parent_cm_id, "");
    assert!(resp.verify_url.is_empty());

    let reply = api
        .add_comment(107192080, "回复", Some("1!PARENT"), &cred())
        .await
        .unwrap();
    assert_eq!(reply.comment_id, "1!CHILD");
    assert_eq!(reply.parent_cm_id, "1!PARENT");
}

/// add_comment 需登录：缺有效凭证 → AuthenticationRequired（不发请求）。
#[tokio::test]
async fn add_comment_requires_credential() {
    let server = MockServer::start().await;
    let client = client_for(&server.uri());
    let api = CommentApi::new(&client);
    assert!(matches!(
        api.add_comment(1, "x", None, &Credential::default()).await,
        Err(QqMusicError::AuthenticationRequired)
    ));
}

/// 回归锚点（2026-09-29 修复）：DelComment 判定键为**内层 `data.Subcode`**
/// （服务端大小写；此前读子响应顶层 `SubCode` 导致恒为 `false`）。
#[tokio::test]
async fn delete_comment_reads_inner_data_subcode() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.globalComment.CommentWriteServer",
            "DelComment",
            |param| param["CommentId"] == json!("1!ABC"),
        ))
        .respond_with(ok_sub(json!({"Subcode": 0, "Msg": ""})))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = CommentApi::new(&client);
    assert!(api.delete_comment("1!ABC", &cred()).await.unwrap());
}

/// 兼容上游读取键 `SubCode`（AddComment 侧大小写）也可判定。
#[tokio::test]
async fn delete_comment_accepts_camel_case_sub_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok_sub(json!({"SubCode": 0, "Msg": ""})))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = CommentApi::new(&client);
    assert!(api.delete_comment("1!ABC", &cred()).await.unwrap());
}

/// `Subcode != 0` → false。
#[tokio::test]
async fn delete_comment_nonzero_subcode_is_false() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok_sub(json!({"Subcode": 20004, "Msg": "无权限"})))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = CommentApi::new(&client);
    assert!(!api.delete_comment("1!ABC", &cred()).await.unwrap());
}

/// 键缺省按上游 `data.get("SubCode", 0)` 语义视为 0 → true
/// （上游语义：评论不存在也返回 true）。
#[tokio::test]
async fn delete_comment_missing_subcode_defaults_true() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok_sub(json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = CommentApi::new(&client);
    assert!(api.delete_comment("1!NOT_EXIST", &cred()).await.unwrap());
}

/// delete_comment 需登录：缺有效凭证 → AuthenticationRequired（不发请求）。
#[tokio::test]
async fn delete_comment_requires_credential() {
    let server = MockServer::start().await;
    let client = client_for(&server.uri());
    let api = CommentApi::new(&client);
    assert!(matches!(
        api.delete_comment("1!ABC", &Credential::default()).await,
        Err(QqMusicError::AuthenticationRequired)
    ));
}
