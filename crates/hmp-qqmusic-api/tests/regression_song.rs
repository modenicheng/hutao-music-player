//! song/lyric/singer 域回归测试（wiremock 离线）。
//!
//! 每个用例锚定一个 2026-09-29 现场实测结论（探针：
//! `examples/live_song.rs`），防止移植回退：
//! - `get_detail` 的发行公司/流派/简介/语言/发布时间在**内层
//!   `data.info.<字段>.content`**（上游 jsonpath `$.info.<字段>.content`），
//!   不提取则恒为空列表；
//! - 取流文件名带 `media_mid`（官方客户端形态）；doubled song-mid
//!   文件名服务端签发 vkey 但 CDN 404；
//! - 明文取流走 `music.vkey.GetVkey`/`UrlGetVkey`，加密取流自动切
//!   `music.vkey.GetEVkey`/`CgiGetEVkey` 并返回 `ekey`；
//! - 完整音质免登录 `104003`（无权限），试听 `TRY` 免登录可用；
//! - `GetSingerDetail`（歌手简介）布尔参数必须 0/1 整数编码
//!   （JSON `true` → 服务端 10006）；
//! - `get_info`/`get_tab_detail` 走 Android comm（ct=11/cv=14090008）；
//! - 歌手歌曲列表数据在 `$.songList[*].songInfo`，专辑列表键 `albumList`，
//!   MV 列表键 `list`；
//! - `get_lyric` 固定 `crypt=1`，加密 QRC hex 自动解密（QRC XML/LRC）。

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::config::ClientConfig;
use hmp_qqmusic_api::credential::{Credential, LoginType};
use hmp_qqmusic_api::error::QqMusicError;
use hmp_qqmusic_api::lyric::LyricApi;
use hmp_qqmusic_api::pagination::Page;
use hmp_qqmusic_api::singer::{
    AreaType, GenreType, IndexType, SexType, SingerApi, TabType,
};
use hmp_qqmusic_api::song::{SongApi, SongFileInfo, SongFileType, SongQueryInfo};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

const SONG_MID: &str = "001qHjVZ4SfmWQ";
const MEDIA_MID: &str = "003BEgWZ2eI1Qo";
const SINGER_MID: &str = "0025NhlN2yWrP4";
const NUMERIC_UIN: &str = "939861972";

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
        str_musicid: NUMERIC_UIN.into(),
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
) -> impl Match {
    move |req: &Request| {
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

/// 校验请求级 comm 覆盖（Android 平台 ct/cv）。
fn comm_is_android() -> impl Match {
    move |req: &Request| {
        let body: serde_json::Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(_) => return false,
        };
        body["comm"]["ct"] == json!(11) && body["comm"]["cv"] == json!(14090008)
    }
}

fn ok_sub(data: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"code": 0, "req_0": {"code": 0, "data": data}}))
}

// ---------- song.get_detail ----------

/// 回归锚点（2026-09-29 修复）：`info.<字段>.content` 必须提取到
/// 发行公司/流派/简介/语言/发布时间（此前在 data 顶层找键，恒为空）。
#[tokio::test]
async fn song_detail_extracts_info_sections() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.pf_song_detail_svr",
            "get_song_detail_yqq",
            |param| param["song_id"] == json!(186016),
        ))
        .respond_with(ok_sub(json!({
            "info": {
                "company": {"title": "发行公司", "content": [
                    {"id": 3, "value": "华纳唱片", "show_type": 0, "jumpurl": ""}
                ]},
                "genre": {"title": "流派", "content": [
                    {"id": 7, "value": "流行", "show_type": 0, "jumpurl": ""}
                ]},
                "intro": {"title": "简介", "content": [
                    {"id": 1, "value": "歌曲简介", "show_type": 0, "jumpurl": ""}
                ]},
                "lan": {"title": "语言", "content": [
                    {"id": 0, "value": "国语", "show_type": 0, "jumpurl": ""}
                ]},
                "pub_time": {"title": "发行时间", "content": [
                    {"id": 0, "value": "2001-07-01", "show_type": 0, "jumpurl": ""}
                ]}
            },
            "extras": {"name": "开始懂了", "wikiurl": ""},
            "track_info": {
                "id": 186016, "mid": SONG_MID, "name": "开始懂了", "type": 0,
                "singer": [{"id": 4558, "mid": "0025NhlN2yWrP4", "name": "孙燕姿"}],
                "album": {"id": 15995, "mid": "002eFUFm2XYZ7z", "name": "风筝"},
                "interval": 267,
                "file": {"media_mid": MEDIA_MID, "size_128mp3": 4358278}
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SongApi::new(&client);
    let resp = api.get_detail("186016").await.unwrap();
    assert_eq!(resp.track.id, 186016);
    assert_eq!(resp.track.mid, SONG_MID);
    assert_eq!(resp.company.len(), 1, "info.company.content 应提取");
    assert_eq!(resp.company[0].value, "华纳唱片");
    assert_eq!(resp.genre[0].value, "流行");
    assert_eq!(resp.intro[0].value, "歌曲简介");
    assert_eq!(resp.lan[0].value, "国语");
    assert_eq!(resp.pub_time[0].value, "2001-07-01");
}

/// 纯数字参数走 `song_id`，否则走 `song_mid`。
#[tokio::test]
async fn song_detail_by_mid_uses_song_mid_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.pf_song_detail_svr",
            "get_song_detail_yqq",
            |param| param["song_mid"] == json!(SONG_MID) && param.get("song_id").is_none(),
        ))
        .respond_with(ok_sub(json!({
            "track_info": {"id": 186016, "mid": SONG_MID, "name": "开始懂了"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SongApi::new(&client);
    let resp = api.get_detail(SONG_MID).await.unwrap();
    assert_eq!(resp.track.id, 186016);
}

// ---------- song.query_song ----------

/// CgiGetTrackInfo 参数形态（ctx/client/types/modify_stamp/mids）与
/// `data.tracks` 解析。
#[tokio::test]
async fn query_song_sends_param_shape_and_parses_tracks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.trackInfo.UniformRuleCtrl",
            "CgiGetTrackInfo",
            |param| {
                param["ctx"] == json!(0)
                    && param["client"] == json!(1)
                    && param["types"] == json!([0])
                    && param["modify_stamp"] == json!([0])
                    && param["mids"] == json!([SONG_MID])
                    && param.get("ids").is_none()
            },
        ))
        .respond_with(ok_sub(json!({
            "tracks": [{"id": 186016, "mid": SONG_MID, "name": "开始懂了", "type": 0}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SongApi::new(&client);
    let tracks = api
        .query_song(&[SongQueryInfo {
            id: None,
            mid: Some(SONG_MID.into()),
            song_type: 0,
        }])
        .await
        .unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id, 186016);
    assert_eq!(tracks[0].name, "开始懂了");
}

/// id 与 mid 混合传参/全空 → InvalidResponse（上游 ValueError 语义）。
#[tokio::test]
async fn query_song_rejects_bad_song_query_info() {
    let server = MockServer::start().await;
    let client = client_for(&server.uri());
    let api = SongApi::new(&client);
    let both = SongQueryInfo {
        id: Some(186016),
        mid: Some(SONG_MID.into()),
        song_type: 0,
    };
    assert!(matches!(
        api.query_song(&[both]).await,
        Err(QqMusicError::InvalidResponse(_))
    ));
    let neither = SongQueryInfo {
        id: None,
        mid: None,
        song_type: 0,
    };
    assert!(matches!(
        api.query_song(&[neither]).await,
        Err(QqMusicError::InvalidResponse(_))
    ));
    assert!(matches!(
        api.query_song(&[]).await,
        Err(QqMusicError::InvalidResponse(_))
    ));
}

// ---------- song.get_song_urls ----------

/// 明文试听取流：UrlGetVkey + media_mid 单文件名；`midurlinfo` 别名解析；
/// build_urls 拼 CDN 域名。
#[tokio::test]
async fn try_urls_use_media_mid_filename_and_parse_midurlinfo() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.vkey.GetVkey",
            "UrlGetVkey",
            |param| {
                param["filename"] == json!([format!("RS02{MEDIA_MID}.mp3")])
                    && param["songmid"] == json!([SONG_MID])
                    && param["songtype"] == json!([0])
                    && param["ctx"] == json!(0)
                    && param["uin"] == json!("")
                    && param["guid"].is_string()
            },
        ))
        .respond_with(ok_sub(json!({
            "expiration": 7200,
            "midurlinfo": [{
                "songmid": SONG_MID,
                "filename": format!("RS02{MEDIA_MID}.mp3"),
                "purl": "RS02003BEgWZ2eI1Qo.mp3?vkey=abc",
                "vkey": "abc",
                "ekey": "",
                "result": 0
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SongApi::new(&client);
    let resp = api
        .get_song_urls(
            &[SongFileInfo {
                mid: SONG_MID.into(),
                file_type: None,
                song_type: 0,
                media_mid: Some(MEDIA_MID.into()),
            }],
            SongFileType::TRY,
            None,
        )
        .await
        .unwrap();
    assert_eq!(resp.expiration, 7200);
    assert_eq!(resp.data[0].result, 0);
    assert_eq!(resp.data[0].songmid, SONG_MID);
    let urls = resp.build_urls();
    assert_eq!(
        urls[0].as_deref(),
        Some("https://isure.stream.qqmusic.qq.com/RS02003BEgWZ2eI1Qo.mp3?vkey=abc")
    );
}

/// 缺省 media_mid 时按上游惯例 mid+mid 拼接文件名（2026-09-29 实测：
/// 该形态服务端签发 vkey 但 CDN 404，仅保留上游行为兼容）。
#[tokio::test]
async fn urls_without_media_mid_double_the_song_mid() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.vkey.GetVkey",
            "UrlGetVkey",
            |param| param["filename"] == json!([format!("RS02{SONG_MID}{SONG_MID}.mp3")]),
        ))
        .respond_with(ok_sub(json!({"expiration": 7200, "midurlinfo": []})))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SongApi::new(&client);
    api.get_song_urls(
        &[SongFileInfo {
            mid: SONG_MID.into(),
            file_type: None,
            song_type: 0,
            media_mid: None,
        }],
        SongFileType::TRY,
        None,
    )
    .await
    .unwrap();
}

/// 加密取流自动切 CgiGetEVkey，登录态注入 str_musicid，响应带 ekey。
#[tokio::test]
async fn encrypted_flac_uses_evkey_with_credential() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.vkey.GetEVkey",
            "CgiGetEVkey",
            |param| {
                param["filename"] == json!([format!("F0M0{MEDIA_MID}.mflac")])
                    && param["uin"] == json!(NUMERIC_UIN)
            },
        ))
        .respond_with(ok_sub(json!({
            "expiration": 80400,
            "midurlinfo": [{
                "songmid": SONG_MID,
                "filename": format!("F0M0{MEDIA_MID}.mflac"),
                "purl": "F0M0003BEgWZ2eI1Qo.mflac?vkey=xyz",
                "vkey": "xyz",
                "ekey": "e.key-364-chars",
                "result": 0
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SongApi::new(&client);
    let resp = api
        .get_song_urls(
            &[SongFileInfo {
                mid: SONG_MID.into(),
                file_type: None,
                song_type: 0,
                media_mid: Some(MEDIA_MID.into()),
            }],
            SongFileType::FLAC,
            Some(&cred()),
        )
        .await
        .unwrap();
    assert_eq!(resp.data[0].result, 0);
    assert_eq!(resp.data[0].ekey, "e.key-364-chars");
}

/// 回归锚点（2026-09-29 实测）：明文完整音质免登录 `result=104003`
/// 且 `purl` 为空 → build_urls 为 None。
#[tokio::test]
async fn mp3_128_anon_104003_builds_no_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok_sub(json!({
            "expiration": 0,
            "midurlinfo": [{
                "songmid": SONG_MID,
                "filename": format!("M500{SONG_MID}{SONG_MID}.mp3"),
                "purl": "",
                "vkey": "",
                "ekey": "",
                "result": 104003
            }]
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SongApi::new(&client);
    let resp = api
        .get_song_urls(
            &[SongFileInfo {
                mid: SONG_MID.into(),
                file_type: None,
                song_type: 0,
                media_mid: None,
            }],
            SongFileType::MP3_128,
            None,
        )
        .await
        .unwrap();
    assert_eq!(resp.data[0].result, 104003);
    assert!(resp.build_urls()[0].is_none());
}

/// item 级 `file_type` 覆盖请求级类型（上游 `item.file_type or file_type`）。
#[tokio::test]
async fn item_file_type_overrides_request_level() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            // 请求级 TRY（明文）不变，item 覆盖仅影响文件名
            "music.vkey.GetVkey",
            "UrlGetVkey",
            |param| param["filename"] == json!([format!("M800{MEDIA_MID}.mp3")]),
        ))
        .respond_with(ok_sub(json!({"expiration": 0, "midurlinfo": []})))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SongApi::new(&client);
    api.get_song_urls(
        &[SongFileInfo {
            mid: SONG_MID.into(),
            file_type: Some(SongFileType::MP3_320),
            song_type: 0,
            media_mid: Some(MEDIA_MID.into()),
        }],
        SongFileType::TRY,
        None,
    )
    .await
    .unwrap();
}

// ---------- lyric.get_lyric ----------

/// 回归锚点：`get_lyric` 固定 `crypt=1`；`qrc` 等开关为 0/1 整数、
/// `needSingingAnnotations` 保留 JSON bool（上游 preserve_bool 语义）；
/// 纯数字 value 走 `songId`；加密 hex 自动解密。
#[tokio::test]
async fn lyric_sends_crypt_params_and_decrypts_qrc() {
    // 用已录制的加密 QRC 样本（fixture）作为 mock 响应体
    let fixture_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/lyric/encrypted.json"
    );
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture_path).unwrap()).unwrap();
    let encrypted = fixture["req_0"]["data"]["lyric"].as_str().unwrap().to_owned();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musichallSong.PlayLyricInfo",
            "GetPlayLyricInfo",
            |param| {
                param["songId"] == json!(186016)
                    && param["crypt"] == json!(1)
                    && param["qrc"] == json!(1)
                    && param["trans"] == json!(0)
                    && param["roma"] == json!(0)
                    && param["needSingingAnnotations"] == json!(false)
                    && param["type"] == json!(0)
            },
        ))
        .respond_with(ok_sub(json!({
            "songID": 186016,
            "qrc": 0,
            "crypt": 1,
            "lyric": encrypted,
            "trans": "",
            "roma": "",
            "lrc_t": 1725958447i64,
            "qrc_t": 0
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = LyricApi::new(&client);
    let resp = api.get_lyric("186016", 0, true, false, false, false).await.unwrap();
    assert_eq!(resp.songid, 186016);
    assert!(
        resp.lyric.contains('[') || resp.lyric.contains("ti"),
        "加密 lyric 应解密为可读内容，实际前缀: {:?}",
        &resp.lyric[..resp.lyric.len().min(40)]
    );
}

/// 非数字 value 走 `songMid`。
#[tokio::test]
async fn lyric_by_mid_uses_song_mid_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musichallSong.PlayLyricInfo",
            "GetPlayLyricInfo",
            |param| param["songMid"] == json!(SONG_MID) && param.get("songId").is_none(),
        ))
        .respond_with(ok_sub(json!({"songID": 186016, "lyric": "[ti:test]\n[00:01.00]x"})))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = LyricApi::new(&client);
    let resp = api.get_lyric(SONG_MID, 0, false, false, false, false).await.unwrap();
    assert_eq!(resp.lyric, "[ti:test]\n[00:01.00]x", "明文 LRC 应原样保留");
}

// ---------- singer ----------

/// GetSingerList 参数（hastag=0 + 三个 -100）与 singerlist/hotlist 解析。
#[tokio::test]
async fn singer_list_sends_filters_and_parses() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musichallSinger.SingerList",
            "GetSingerList",
            |param| {
                param["hastag"] == json!(0)
                    && param["area"] == json!(-100)
                    && param["sex"] == json!(-100)
                    && param["genre"] == json!(7)
            },
        ))
        .respond_with(ok_sub(json!({
            "area": -100, "sex": -100, "genre": 7,
            "singerlist": [{"singer_id": 4558, "singer_mid": SINGER_MID,
                            "singer_name": "周杰伦", "concernNum": 1}],
            "code": 0,
            "hotlist": [],
            "tags": {}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SingerApi::new(&client);
    let resp = api
        .get_singer_list(AreaType::All, SexType::All, GenreType::Pop)
        .await
        .unwrap();
    assert_eq!(resp.code, 0);
    assert_eq!(resp.singerlist.len(), 1);
    assert_eq!(resp.singerlist[0].id, 4558);
    assert_eq!(resp.singerlist[0].mid, SINGER_MID);
    assert_eq!(resp.singerlist[0].name, "周杰伦");
    assert!(resp.tags.area.is_empty(), "tags={{}} 应规整为空列表");
}

/// GetSingerListIndex 参数（index/sin/cur_page）与 flatten 解析。
#[tokio::test]
async fn singer_list_index_sends_pagination_and_parses_total() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musichallSinger.SingerList",
            "GetSingerListIndex",
            |param| {
                param["index"] == json!(27)
                    && param["sin"] == json!(80)
                    && param["cur_page"] == json!(2)
            },
        ))
        .respond_with(ok_sub(json!({
            "singerlist": [{"singer_id": 1, "singer_mid": "m", "singer_name": "a"}],
            "code": 0,
            "index": 27,
            "total": 6803
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SingerApi::new(&client);
    let resp = api
        .get_singer_list_index(
            AreaType::All,
            SexType::All,
            GenreType::All,
            IndexType::Hash,
            Page::new(2, 80),
        )
        .await
        .unwrap();
    assert_eq!(resp.base.singerlist.len(), 1);
    assert_eq!(resp.total, 6803);
}

/// 回归锚点：get_info 走 Android comm（ct=11/cv=14090008），
/// `Info.Singer`/`Info.BaseInfo` 提取到顶层字段。
#[tokio::test]
async fn singer_info_uses_android_comm_and_extracts_info() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(comm_is_android())
        .and(req_matches(
            "music.UnifiedHomepage.UnifiedHomepageSrv",
            "GetHomepageHeader",
            |param| param["SingerMid"] == json!(SINGER_MID),
        ))
        .respond_with(ok_sub(json!({
            "Status": 0,
            "Info": {
                "Singer": {"SingerID": 4558, "SingerMid": SINGER_MID, "Name": "周杰伦"},
                "BaseInfo": {"Name": "周杰伦", "Avatar": "https://img.example/a"}
            },
            "TabDetail": {"TabID": "wiki"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SingerApi::new(&client);
    let resp = api.get_info(SINGER_MID).await.unwrap();
    assert_eq!(resp.status, 0);
    assert_eq!(resp.singer.id, 4558);
    assert_eq!(resp.singer.name, "周杰伦");
    assert_eq!(resp.base_info.name, "周杰伦");
}

/// 回归锚点：get_tab_detail 走 Android comm；SongTab.List 提取到 song_tab。
#[tokio::test]
async fn singer_tab_detail_uses_android_comm_and_extracts_song_tab() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(comm_is_android())
        .and(req_matches(
            "music.UnifiedHomepage.UnifiedHomepageSrv",
            "GetHomepageTabDetail",
            |param| {
                param["SingerMid"] == json!(SINGER_MID)
                    && param["IsQueryTabDetail"] == json!(1)
                    && param["TabID"] == json!("song_sing")
                    && param["PageNum"] == json!(0)
                    && param["PageSize"] == json!(5)
                    && param["Order"] == json!(0)
            },
        ))
        .respond_with(ok_sub(json!({
            "TabID": "song_sing",
            "HasMore": 1,
            "SongTab": {"List": [
                {"id": 97773, "mid": "0039MnYb0qxYhV", "name": "晴天", "type": 0}
            ]}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SingerApi::new(&client);
    let resp = api
        .get_tab_detail(SINGER_MID, TabType::Song, Page::new(1, 5))
        .await
        .unwrap();
    assert_eq!(resp.tab_id, "song_sing");
    assert_eq!(resp.has_more, 1);
    assert_eq!(resp.song_tab.len(), 1);
    assert_eq!(resp.song_tab[0].name, "晴天");
}

/// 回归锚点（2026-09-29 实测）：GetSingerDetail 布尔参数必须 0/1 整数
/// （JSON `true` → 服务端 10006）。
#[tokio::test]
async fn singer_desc_sends_bools_as_ints() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musichallSinger.SingerInfoInter",
            "GetSingerDetail",
            |param| {
                param["singer_mids"] == json!([SINGER_MID])
                    && param["group_singer"] == json!(1)
                    && param["wiki_singer"] == json!(1)
                    && param["ex_singer"] == json!(1)
                    && param["pic"] == json!(1)
                    && param["photos"] == json!(0)
            },
        ))
        .respond_with(ok_sub(json!({
            "singer_list": [{
                "basic_info": {"singer_id": 4558, "singer_mid": SINGER_MID, "name": "周杰伦"},
                "ex_info": {"desc": "歌手简介"}
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SingerApi::new(&client);
    let resp = api
        .get_desc(&[SINGER_MID.to_string()], true, true, true, true, false)
        .await
        .unwrap();
    assert_eq!(resp.singer_list.len(), 1);
    assert_eq!(resp.singer_list[0].basic_info.name, "周杰伦");
    assert_eq!(resp.singer_list[0].ex_info.desc, "歌手简介");
}

/// 回归锚点：歌手歌曲列表数据在 `$.songList[*].songInfo`。
#[tokio::test]
async fn singer_songs_extracts_song_info() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "musichall.song_list_server",
            "GetSingerSongList",
            |param| {
                param["singerMid"] == json!(SINGER_MID)
                    && param["order"] == json!(1)
                    && param["number"] == json!(5)
                    && param["begin"] == json!(0)
            },
        ))
        .respond_with(ok_sub(json!({
            "singerMid": SINGER_MID,
            "totalNum": 1012,
            "songList": [
                {"songInfo": {"id": 97773, "mid": "0039MnYb0qxYhV", "name": "晴天", "type": 0}}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SingerApi::new(&client);
    let resp = api.get_songs_list(SINGER_MID, Page::new(1, 5)).await.unwrap();
    assert_eq!(resp.singer_mid, SINGER_MID);
    assert_eq!(resp.total_num, 1012);
    assert_eq!(resp.song_list.len(), 1);
    assert_eq!(resp.song_list[0].name, "晴天");
}

/// 专辑列表键 `albumList`（AlbumBrief.totalNum 别名）。
#[tokio::test]
async fn singer_albums_parse_album_list_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musichallAlbum.AlbumListServer",
            "GetAlbumList",
            |param| {
                param["singerMid"] == json!(SINGER_MID)
                    && param["begin"] == json!(5)
            },
        ))
        .respond_with(ok_sub(json!({
            "singerMid": SINGER_MID,
            "total": 43,
            "albumList": [
                {"albumID": 98093, "albumMid": "002eFUFm2XYZ7z", "albumName": "风筝",
                 "totalNum": 10, "singerName": "孙燕姿", "albumType": "录音室专辑", "tags": null}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SingerApi::new(&client);
    let resp = api.get_album_list(SINGER_MID, Page::new(2, 5)).await.unwrap();
    assert_eq!(resp.total, 43);
    assert_eq!(resp.album_list.len(), 1);
    assert_eq!(resp.album_list[0].album.name, "风筝");
    assert_eq!(resp.album_list[0].total_num, 10);
    assert!(resp.album_list[0].tags.is_empty(), "tags=null 应规整为空列表");
}

/// MV 列表键 `list` → mv_list；VideoBrief.mvid 别名。
#[tokio::test]
async fn singer_mvs_parse_list_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "MvService.MvInfoProServer",
            "GetSingerMvList",
            |param| {
                param["singermid"] == json!(SINGER_MID)
                    && param["count"] == json!(5)
                    && param["start"] == json!(0)
            },
        ))
        .respond_with(ok_sub(json!({
            "total": 10426,
            "list": [
                {"mvid": 1, "vid": "w0026q7f01a", "title": "MV", "type": 1,
                 "picurl": "https://img.example/p", "playcnt": 100}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SingerApi::new(&client);
    let resp = api.get_mv_list(SINGER_MID, Page::new(1, 5)).await.unwrap();
    assert_eq!(resp.total, 10426);
    assert_eq!(resp.mv_list.len(), 1);
    assert_eq!(resp.mv_list[0].vid, "w0026q7f01a");
    assert_eq!(resp.mv_list[0].id, 1);
}

/// 相似歌手：`singerId`/`singerMid`/`singerName`/`pic_mid` 别名解析。
#[tokio::test]
async fn similar_singers_parse_aliases() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.SimilarSingerSvr",
            "GetSimilarSingerList",
            |param| {
                param["singerMid"] == json!(SINGER_MID) && param["number"] == json!(5)
            },
        ))
        .respond_with(ok_sub(json!({
            "singerlist": [
                {"singerId": 4558, "singerMid": SINGER_MID, "singerName": "周杰伦",
                 "pic_mid": "pmid1", "singerPic": "https://img.example/s"}
            ],
            "code": 0,
            "errMsg": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let api = SingerApi::new(&client);
    let resp = api.get_similar(SINGER_MID, 5).await.unwrap();
    assert_eq!(resp.code, 0);
    assert_eq!(resp.singerlist.len(), 1);
    assert_eq!(resp.singerlist[0].id, 4558);
    assert_eq!(resp.singerlist[0].mid, SINGER_MID);
    assert_eq!(resp.singerlist[0].pmid, "pmid1");
}
