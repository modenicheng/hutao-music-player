//! user/songlist 域回归测试（wiremock 离线）。
//!
//! 每个用例锚定一个 2026-09-29 现场实测结论（探针：
//! `examples/live_songlist.rs`），防止移植回退：
//! - `GetPlaylistByUin` 必须传数字 uin（加密 uin → 服务端 80030）；
//! - `CgiGetDiss` 的 `enc_host_uin` 用加密 uin；
//! - `PlaylistFavRead` 参数键 `uin`（加密 uin）、`AlbumFavRead` 参数键 `euin`，
//!   列表数据键均为 `v_list`；
//! - `PlaylistFavWrite` 响应的 `result`/`v_failedPlaylistId` 在**内层 data**
//!   （上游 `_build_cgi` 无 response_model 时返回内层 data）；
//! - `AddSonglist`/`DelSonglist` 的 CGI 错误 80092 按上游语义映射为 `Ok(false)`。

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::config::ClientConfig;
use hmp_qqmusic_api::credential::{Credential, LoginType};
use hmp_qqmusic_api::error::QqMusicError;
use hmp_qqmusic_api::pagination::Page;
use hmp_qqmusic_api::songlist::SonglistApi;
use hmp_qqmusic_api::user::UserApi;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NUMERIC_UIN: &str = "939861972";
const ENCRYPT_UIN: &str = "NKoqNeC5NKSA";

fn client_for(base_url: &str) -> QqMusicClient {
    let config = ClientConfig {
        base_url: base_url.to_owned(),
        ..Default::default()
    };
    QqMusicClient::with_config(config)
}

fn cred() -> Credential {
    Credential {
        uin: NUMERIC_UIN.into(),
        music_id: NUMERIC_UIN.into(),
        music_key: "test-key".into(),
        login_type: LoginType::Qq,
        encrypt_uin: ENCRYPT_UIN.into(),
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

// ---------- user：读接口 ----------

/// GetPlaylistByUin 参数必须是数字 uin；v_playlist/bFinish/v_delTid 可解析。
#[tokio::test]
async fn created_songlist_sends_numeric_uin_and_parses_v_playlist() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistBaseRead",
            "GetPlaylistByUin",
            |param| param["uin"] == json!(NUMERIC_UIN),
        ))
        .respond_with(ok_sub(json!({
            "total": 2,
            "v_playlist": [
                {"tid": 9785418994i64, "dirId": 12, "dissname": "HMP-API-TEST-1", "songnum": 1},
                {"tid": 123, "dirId": 13, "dissname": "日常"}
            ],
            "bFinish": true,
            "v_delTid": [99],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    let resp = api
        .get_created_songlist(NUMERIC_UIN, Some(&cred()))
        .await
        .unwrap();
    assert_eq!(resp.total, 2);
    assert_eq!(resp.songlist.len(), 2);
    assert_eq!(resp.songlist[0].id, 9785418994);
    assert_eq!(resp.songlist[0].title, "HMP-API-TEST-1");
    assert!(resp.finished);
    assert_eq!(resp.deleted_ids, vec![99]);
}

/// 回归锚点（2026-09-29 实测）：GetPlaylistByUin 传加密 uin → 服务端 80030。
#[tokio::test]
async fn created_songlist_with_encrypted_uin_maps_80030_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 80030, "data": {}}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    let err = api
        .get_created_songlist(ENCRYPT_UIN, Some(&cred()))
        .await
        .unwrap_err();
    match err {
        QqMusicError::QqApi { code, .. } => assert_eq!(code, 80030),
        other => panic!("expected QqApi(80030), got {other:?}"),
    }
}

/// CgiGetDiss（dirid=201）的 enc_host_uin 必须是加密 uin。
#[tokio::test]
async fn fav_song_sends_enc_host_uin_and_parses_detail() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.srfDissInfo.DissInfo",
            "CgiGetDiss",
            |param| {
                param["dirid"] == json!(201)
                    && param["disstid"] == json!(0)
                    && param["enc_host_uin"] == json!(ENCRYPT_UIN)
                    && param["song_begin"] == json!(0)
                    && param["song_num"] == json!(2)
            },
        ))
        .respond_with(ok_sub(json!({
            "code": 0,
            "dirinfo": {"dissid": 0, "dissname": "我喜欢", "songnum": 801},
            "songlist": [
                {"id": 186016, "mid": "001qHjVZ4SfmWQ", "name": "开始懂了", "type": 0}
            ],
            "songlist_size": 1,
            "total_song_num": 801,
            "hasmore": 1,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    let resp = api
        .get_fav_song(ENCRYPT_UIN, Page::new(1, 2), Some(&cred()))
        .await
        .unwrap();
    assert_eq!(resp.total, 801);
    assert_eq!(resp.songs.len(), 1);
    assert_eq!(resp.songs[0].id, 186016);
    assert_eq!(resp.songs[0].mid, "001qHjVZ4SfmWQ");
    assert_eq!(resp.hasmore, 1);
}

/// 回归锚点（2026-09-29 实测）：PlaylistFavRead 参数键 `uin`（加密 uin），
/// 列表数据键 `v_list`（旧别名 vecSonglist 不存在 → 此前解析为 0 条）。
#[tokio::test]
async fn fav_songlist_sends_uin_key_and_parses_v_list() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistFavRead",
            "CgiGetPlaylistFavInfo",
            |param| {
                param["uin"] == json!(ENCRYPT_UIN)
                    && param["offset"] == json!(0)
                    && param["size"] == json!(10)
            },
        ))
        .respond_with(ok_sub(json!({
            "number": 10,
            "hasmore": 1,
            "v_list": [
                {"tid": 7843129912i64, "dirId": 7, "name": "华语精选", "songnum": 30,
                 "logo": "https://img.example/logo", "nickname": "路人"}
            ],
            "total": 12,
            "v_delTids": [],
            "v_failTids": [5],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    let resp = api
        .get_fav_songlist(ENCRYPT_UIN, Page::new(1, 10), Some(&cred()))
        .await
        .unwrap();
    assert_eq!(resp.total, 12);
    assert_eq!(resp.playlists.len(), 1, "v_list 应解析出歌单");
    assert_eq!(resp.playlists[0].id, 7843129912i64);
    assert_eq!(resp.playlists[0].dirid, 7);
    assert_eq!(resp.failed_ids, vec![5]);
}

/// 回归锚点（2026-09-29 实测）：AlbumFavRead 参数键是 `euin`，列表数据键 `v_list`。
#[tokio::test]
async fn fav_album_sends_euin_key_and_parses_v_list() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.AlbumFavRead",
            "CgiGetAlbumFavInfo",
            |param| {
                param["euin"] == json!(ENCRYPT_UIN)
                    && param["offset"] == json!(10)
                    && param["size"] == json!(10)
            },
        ))
        .respond_with(ok_sub(json!({
            "number": 10,
            "hasmore": 1,
            "v_list": [
                {"albumID": 15995, "albumMid": "004YCGQa3qOG8u", "albumName": "经典全纪录"}
            ],
            "total": 32,
            "v_failAlbumId": [],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    let resp = api
        .get_fav_album(ENCRYPT_UIN, Page::new(2, 10), Some(&cred()))
        .await
        .unwrap();
    assert_eq!(resp.total, 32);
    assert_eq!(resp.albums.len(), 1);
    assert_eq!(resp.albums[0].id, 15995);
    assert_eq!(resp.albums[0].mid, "004YCGQa3qOG8u");
}

/// GetProfileReport（音乐基因）VisitAccount 传加密 uin，UserInfoCard 可解析。
#[tokio::test]
async fn music_gene_sends_visit_account_and_parses_card() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.recommend.UserProfileSettingSvr",
            "GetProfileReport",
            |param| param["VisitAccount"] == json!(ENCRYPT_UIN),
        ))
        .respond_with(ok_sub(json!({
            "UserInfoCard": {
                "NickName": "程家麒",
                "HeadUrl": "https://img.example/head",
                "Signature": "签名"
            },
            "ListeningReport": {"Report": []},
            "SortArray": [1, 2],
            "IsVisitAccount": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    let gene = api
        .get_music_gene(ENCRYPT_UIN, Some(&cred()))
        .await
        .unwrap();
    assert_eq!(gene.userinfo_card.nick_name, "程家麒");
    assert_eq!(gene.userinfo_card.head_url, "https://img.example/head");
    assert_eq!(gene.userinfo_card.signature, "签名");
}

// ---------- user：收藏歌单写操作（PlaylistFavWrite）----------

/// 回归锚点（2026-09-29 修复）：fav/unfav 的 `result`/`v_failedPlaylistId`
/// 在**内层 data**；不解包内层会导致恒为 `Ok(false)`。
#[tokio::test]
async fn fav_songlist_unwraps_inner_data_and_returns_true() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistFavWrite",
            "FavPlaylist",
            |param| {
                // 参数键 `uin` 传加密 uin，v_playlistId 为 tid 数组
                param["uin"] == json!(ENCRYPT_UIN) && param["v_playlistId"] == json!([42])
            },
        ))
        .respond_with(ok_sub(
            json!({"result": 0, "v_failedPlaylistId": [], "reason": ""}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    assert!(api.fav_songlist(42, &cred()).await.unwrap());
}

/// 服务端拒绝（自建歌单/201 目录）：result=80184 → false（2026-09-29 实测）。
#[tokio::test]
async fn fav_songlist_self_dir_rejected_returns_false() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok_sub(json!({
            "reason": "can't order self's dir or 201 dir",
            "result": 80184,
            "v_failedPlaylistId": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    assert!(!api.fav_songlist(9785418994i64, &cred()).await.unwrap());
}

/// 取消收藏：未收藏的歌单服务端 result=0 → true（2026-09-29 实测）。
#[tokio::test]
async fn unfav_songlist_returns_true_for_not_favorited() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistFavWrite",
            "CancelFavPlaylist",
            |param| param["uin"] == json!(ENCRYPT_UIN) && param["v_playlistId"] == json!([42]),
        ))
        .respond_with(ok_sub(
            json!({"result": 0, "v_failedPlaylistId": [], "reason": ""}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    assert!(api.unfav_songlist(42, &cred()).await.unwrap());
}

/// 目标歌单进入 v_failedPlaylistId → false（上游判定逻辑）。
#[tokio::test]
async fn fav_songlist_failed_list_contains_target_returns_false() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok_sub(json!({"result": 0, "v_failedPlaylistId": [42]})))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    assert!(!api.fav_songlist(42, &cred()).await.unwrap());
}

// ---------- songlist：写闭环 ----------

/// 创建歌单：param dirName；`$.result.{tid,dirId,dirName}` 提取。
#[tokio::test]
async fn create_sends_dirname_and_extracts_result_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistBaseWrite",
            "AddPlaylist",
            |param| param["dirName"] == json!("HMP-API-TEST-1790650040"),
        ))
        .respond_with(ok_sub(json!({
            "retCode": 0,
            "result": {"tid": 9785418994i64, "dirId": 12, "dirName": "HMP-API-TEST-1790650040"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SonglistApi::new(&client);
    let resp = api
        .create("HMP-API-TEST-1790650040", &cred())
        .await
        .unwrap();
    assert_eq!(resp.ret_code, 0);
    assert_eq!(resp.id, 9785418994i64);
    assert_eq!(resp.dirid, 12);
    assert_eq!(resp.name, "HMP-API-TEST-1790650040");
}

/// 删除歌单：param dirId。
#[tokio::test]
async fn delete_sends_dirid() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistBaseWrite",
            "DelPlaylist",
            |param| param["dirId"] == json!(12),
        ))
        .respond_with(ok_sub(json!({
            "retCode": 0,
            "result": {"tid": 9785418994i64, "dirId": 12, "dirName": "HMP-API-TEST-1"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SonglistApi::new(&client);
    let resp = api.delete(12, &cred()).await.unwrap();
    assert_eq!(resp.ret_code, 0);
    assert_eq!(resp.dirid, 12);
}

/// 加歌：param 含 dirId/tid/bFmtUtf8/v_songInfo[{songId,songType}]；retCode=0 → true。
#[tokio::test]
async fn add_songs_sends_song_info_and_returns_true() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistDetailWrite",
            "AddSonglist",
            |param| {
                param["dirId"] == json!(12)
                    && param["tid"] == json!(9785418994i64)
                    && param["bFmtUtf8"] == json!(true)
                    && param["v_songInfo"] == json!([{"songId": 186016, "songType": 0}])
            },
        ))
        .respond_with(ok_sub(json!({"retCode": 0})))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SonglistApi::new(&client);
    assert!(
        api.add_songs(12, &[(186016, 0)], 9785418994i64, &cred())
            .await
            .unwrap()
    );
}

/// retCode!=0 → false。
#[tokio::test]
async fn add_songs_nonzero_retcode_returns_false() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok_sub(json!({"retCode": 80092})))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SonglistApi::new(&client);
    assert!(!api.add_songs(12, &[(186016, 0)], 0, &cred()).await.unwrap());
}

/// 回归锚点（上游语义）：CGI 错误 80092 按上游 `add_songs` 捕获逻辑返回
/// `Ok(false)`，而非向上抛 `QqApi(80092)`。
#[tokio::test]
async fn add_songs_cgi_80092_maps_to_false() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 80092, "data": {}}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SonglistApi::new(&client);
    assert!(!api.add_songs(12, &[(186016, 0)], 0, &cred()).await.unwrap());
    assert!(!api.del_songs(12, &[(186016, 0)], 0, &cred()).await.unwrap());
}

/// 其他 CGI 错误码（如 like_song 实测遇到的 80105）应原样抛出。
#[tokio::test]
async fn add_songs_other_cgi_error_propagates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "req_0": {"code": 80105, "data": {}}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SonglistApi::new(&client);
    let err = api.like_song(&[(186016, 0)], &cred()).await.unwrap_err();
    match err {
        QqMusicError::QqApi { code, .. } => assert_eq!(code, 80105),
        other => panic!("expected QqApi(80105), got {other:?}"),
    }
}

/// 删歌：DelSonglist param；retCode=0 → true。
#[tokio::test]
async fn del_songs_returns_true_on_retcode_zero() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistDetailWrite",
            "DelSonglist",
            |param| {
                param["dirId"] == json!(12)
                    && param["v_songInfo"] == json!([{"songId": 186016, "songType": 0}])
            },
        ))
        .respond_with(ok_sub(json!({"retCode": 0})))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SonglistApi::new(&client);
    assert!(api.del_songs(12, &[(186016, 0)], 0, &cred()).await.unwrap());
}

/// like/unlike 固定 dirId=201（「我喜欢」）。
#[tokio::test]
async fn like_and_unlike_song_target_dirid_201() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistDetailWrite",
            "AddSonglist",
            |param| param["dirId"] == json!(201),
        ))
        .respond_with(ok_sub(json!({"retCode": 0})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistDetailWrite",
            "DelSonglist",
            |param| param["dirId"] == json!(201),
        ))
        .respond_with(ok_sub(json!({"retCode": 0})))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SonglistApi::new(&client);
    assert!(api.like_song(&[(186016, 0)], &cred()).await.unwrap());
    assert!(api.unlike_song(&[(186016, 0)], &cred()).await.unwrap());
}

/// get_detail 免登录（None credential）：param disstid/dirid/onlysong 等。
#[tokio::test]
async fn get_detail_works_without_credential() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.srfDissInfo.DissInfo",
            "CgiGetDiss",
            |param| {
                param["disstid"] == json!(9785418994i64)
                    && param["dirid"] == json!(0)
                    && param["onlysonglist"] == json!(false)
                    && param["song_num"] == json!(10)
            },
        ))
        .respond_with(ok_sub(json!({
            "code": 0,
            "dirinfo": {"dissid": 9785418994i64, "dissname": "HMP-API-TEST-1",
                         "creator": {"musicid": 939861972i64, "nick": "测试", "encryptUin": "NKoqNeC5NKSA"}},
            "songlist": [],
            "songlist_size": 0,
            "total_song_num": 0
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SonglistApi::new(&client);
    let resp = api
        .get_detail(9785418994i64, 0, Page::new(1, 10), false, true, true)
        .await
        .unwrap();
    assert_eq!(resp.code, 0);
    assert_eq!(resp.info.list.id, 9785418994i64);
    assert_eq!(resp.info.list.title, "HMP-API-TEST-1");
    assert_eq!(resp.info.creator.nick, "测试");
    assert!(resp.songs.is_empty());
}

/// 需登录写操作缺凭证 → AuthenticationRequired（不发请求）。
#[tokio::test]
async fn write_ops_require_credential() {
    let server = MockServer::start().await;
    // 不挂任何 mock：若发出请求则连接失败
    let client = client_for(&server.uri());
    let songlist = SonglistApi::new(&client);
    let user = UserApi::new(&client);
    let empty: Vec<(i64, i64)> = vec![];
    assert!(matches!(
        songlist.create("x", &Credential::default()).await,
        Err(QqMusicError::AuthenticationRequired)
    ));
    assert!(matches!(
        songlist.delete(1, &Credential::default()).await,
        Err(QqMusicError::AuthenticationRequired)
    ));
    assert!(matches!(
        songlist
            .add_songs(1, &empty, 0, &Credential::default())
            .await,
        Err(QqMusicError::AuthenticationRequired)
    ));
    assert!(matches!(
        songlist.like_song(&empty, &Credential::default()).await,
        Err(QqMusicError::AuthenticationRequired)
    ));
    assert!(matches!(
        user.fav_songlist(1, &Credential::default()).await,
        Err(QqMusicError::AuthenticationRequired)
    ));
    assert!(matches!(
        user.get_vip_info(&Credential::default()).await,
        Err(QqMusicError::AuthenticationRequired)
    ));
}
