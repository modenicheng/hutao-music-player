//! 统一分页回归测试（离线）：`Page`/`PagedView` 原语 + 各响应类型
//! `Paged::paged` 的 has_more 归一规则（wiremock 锚定 wire 参数派生）。
//!
//! 三条归一规则（crate::pagination 模块文档）：
//! 1. 显式 hasmore（收藏歌单/收藏专辑/歌单详情/评论/推荐歌单）以服务端为准；
//! 2. 无显式字段但有总数（歌手列表/歌曲/专辑/MV/新碟/榜单详情）按
//!    `total > offset + len` 推算；
//! 3. 两者皆缺按 `items.len() == page.num` 保守推算（`PagedView::conservative`）。

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::comment::CommentApi;
use hmp_qqmusic_api::config::ClientConfig;
use hmp_qqmusic_api::pagination::{DEFAULT_NUM, MAX_NUM, UNKNOWN_TOTAL, Page, Paged, PagedView};
use hmp_qqmusic_api::singer::{IndexType, SingerApi};
use hmp_qqmusic_api::songlist::SonglistApi;
use hmp_qqmusic_api::user::UserApi;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------- Page：钳制 / offset / next / Display / Default / From ----------

#[test]
fn page_clamps_page_number_and_size() {
    // page 钳制 ≥1；num 钳制 1..=100（QQ 服务端通用上限）。
    assert_eq!(Page::new(0, 0), Page { page: 1, num: 1 });
    assert_eq!(Page::new(0, 500), Page { page: 1, num: MAX_NUM });
    assert_eq!(Page::new(3, 101), Page { page: 3, num: MAX_NUM });
    assert_eq!(Page::new(2, 50), Page { page: 2, num: 50 });
}

#[test]
fn page_offset_is_zero_based_window_start() {
    assert_eq!(Page::first().offset(), 0);
    assert_eq!(Page::new(1, 30).offset(), 0);
    assert_eq!(Page::new(2, 30).offset(), 30);
    assert_eq!(Page::new(3, 7).offset(), 14);
    assert_eq!(Page::new(2, 80).offset(), 80);
}

#[test]
fn page_next_keeps_num_and_saturates() {
    assert_eq!(Page::new(1, 20).next(), Page::new(2, 20));
    assert_eq!(Page::first().next(), Page::new(2, DEFAULT_NUM));
    assert_eq!(
        Page { page: u32::MAX, num: 5 }.next().page,
        u32::MAX,
        "u32 溢出饱和"
    );
}

#[test]
fn page_default_first_from_and_display() {
    assert_eq!(Page::default(), Page::first());
    assert_eq!(Page::first().num, DEFAULT_NUM);
    assert_eq!(Page::from((2, 40)), Page::new(2, 40));
    assert_eq!(Page::new(2, 40).to_string(), "page 2 (40 /page)");
}

// ---------- PagedView：next_page / 保守归一（规则 3） ----------

#[test]
fn paged_view_next_page_follows_has_more() {
    let items = vec![1, 2, 3];
    let page = Page::new(1, 3);
    let view = PagedView {
        items: &items,
        total: 10,
        has_more: true,
        page,
    };
    assert_eq!(view.next_page(), Some(Page::new(2, 3)));
    let done = PagedView {
        items: &items,
        total: 3,
        has_more: false,
        page,
    };
    assert_eq!(done.next_page(), None);
}

#[test]
fn conservative_view_infers_from_full_page() {
    // 规则 3：items.len() == page.num → 假设还有下一页（服务端无 hasmore
    // 也无 total 时的保守推算；满页末页会多取一次空页即终止）。
    let full: Vec<u8> = (0..5).collect();
    let v = PagedView::conservative(&full, Page::new(1, 5));
    assert!(v.has_more);
    assert_eq!(v.total, UNKNOWN_TOTAL);
    assert_eq!(v.next_page(), Some(Page::new(2, 5)));

    let short: Vec<u8> = (0..4).collect();
    let v = PagedView::conservative(&short, Page::new(1, 5));
    assert!(!v.has_more);

    let empty: Vec<u8> = Vec::new();
    assert!(!PagedView::conservative(&empty, Page::new(1, 5)).has_more);
}

// ---------- 规则 1：显式 hasmore（纯 JSON 反序列化） ----------

#[test]
fn fav_songlist_paged_uses_server_hasmore() {
    use hmp_qqmusic_api::user::UserFavSonglistResponse;
    let resp: UserFavSonglistResponse = serde_json::from_value(json!({
        "hasmore": 1,
        "v_list": [{"tid": 111, "dirId": 3, "name": "华语精选"}],
        "total": 12,
    }))
    .unwrap();
    let page = Page::new(1, 10);
    let view = resp.paged(page);
    assert_eq!(view.items.len(), 1);
    assert_eq!(view.total, 12);
    // 显式字段优先：即使 total(12) 已被本页覆盖（offset+len=10 < 12 成立，
    // 反例见下），仍以服务端为准；此处验证正向。
    assert!(view.has_more);
    assert_eq!(view.next_page(), Some(Page::new(2, 10)));

    let last: UserFavSonglistResponse =
        serde_json::from_value(json!({"hasmore": 0, "v_list": [], "total": 12}))
            .unwrap();
    assert!(!last.paged(Page::new(2, 10)).has_more);
}

#[test]
fn songlist_detail_paged_uses_server_hasmore() {
    use hmp_qqmusic_api::songlist::GetSonglistDetailResponse;
    // 显式 hasmore=0 但 total 未覆盖时也不猜（服务端为准）。
    let resp: GetSonglistDetailResponse = serde_json::from_value(json!({
        "hasmore": 0,
        "songlist": [{"songId": 1, "songName": "a"}, {"songId": 2, "songName": "b"}],
        "total_song_num": 100,
    }))
    .unwrap();
    let view = resp.paged(Page::new(1, 2));
    assert_eq!(view.items.len(), 2);
    assert_eq!(view.total, 100);
    assert!(!view.has_more, "显式 hasmore=0 以服务端为准");
}

#[test]
fn recommend_songlist_paged_uses_server_hasmore_without_total() {
    use hmp_qqmusic_api::recommend::RecommendSonglistResponse;
    let resp: RecommendSonglistResponse = serde_json::from_value(json!({
        "HasMore": true,
        "List": [{"Playlist": {"basic": {"tid": 1, "title": "x"}}}],
    }))
    .unwrap();
    let view = resp.paged(Page::new(1, 25));
    assert!(view.has_more);
    assert_eq!(view.total, UNKNOWN_TOTAL, "服务端不返回总数");
}

#[test]
fn comment_list_paged_uses_inner_hasmore_and_total() {
    use hmp_qqmusic_api::comment::CommentListResponse;
    let resp: CommentListResponse = serde_json::from_value(json!({
        "CommentList": {
            "Comments": [{"CmId": "1!A", "Nick": "n", "Content": "c"}],
            "HasMore": 1,
            "Total": 83633,
        },
        "TotalCmNum": 99999,
    }))
    .unwrap();
    let view = resp.paged(Page::new(1, 20));
    assert_eq!(view.items.len(), 1);
    // total 取 CommentList.Total（分页相关），非顶层 TotalCmNum。
    assert_eq!(view.total, 83633);
    assert!(view.has_more);

    let none: CommentListResponse = serde_json::from_value(json!({})).unwrap();
    let view = none.paged(Page::new(1, 20));
    assert!(view.items.is_empty());
    assert!(!view.has_more);
}

// ---------- 规则 2：total 推算（纯 JSON 反序列化） ----------

#[test]
fn singer_index_paged_infers_from_total() {
    use hmp_qqmusic_api::singer::SingerIndexPageResponse;
    let resp: SingerIndexPageResponse = serde_json::from_value(json!({
        "singerlist": [{"singerId": 1}, {"singerId": 2}],
        "total": 6803,
        "index": 27,
    }))
    .unwrap();
    let page = Page::new(1, 80);
    let view = resp.paged(page);
    assert_eq!(view.items.len(), 2);
    assert_eq!(view.total, 6803);
    assert!(view.has_more, "total 6803 > offset 0 + len 2");

    // 边界：offset(6800) + len(2) = 6802 < 6803 → 仍有一项，判定有下一页。
    let view = resp.paged(Page::new(86, 80));
    assert!(view.has_more, "total 6803 > offset 6800 + len 2");
    // offset(6880) + len(2) ≥ total(6803) → 无下一页。
    let view = resp.paged(Page::new(87, 80));
    assert!(!view.has_more, "offset 6880 + len 2 ≥ total 6803");
}

#[test]
fn new_album_paged_infers_from_total() {
    use hmp_qqmusic_api::album::GetNewAlbumResponse;
    let resp: GetNewAlbumResponse = serde_json::from_value(json!({
        "total": 5,
        "albums": [{"albumID": 1}, {"albumID": 2}],
    }))
    .unwrap();
    let view = resp.paged(Page::new(1, 5));
    assert_eq!(view.total, 5);
    assert!(view.has_more, "total 5 > offset 0 + len 2");

    let view = resp.paged(Page::new(2, 5));
    assert!(!view.has_more, "offset 5 + len 2 ≥ total 5 → 无下一页");
}

#[test]
fn top_detail_paged_infers_from_total_num() {
    use hmp_qqmusic_api::top::TopDetailResponse;
    let resp: TopDetailResponse = serde_json::from_value(json!({
        "data": {"topId": 62, "totalNum": 100},
        "songInfoList": [{"songId": 1}, {"songId": 2}, {"songId": 3}],
    }))
    .unwrap();
    let view = resp.paged(Page::new(2, 5));
    assert_eq!(view.items.len(), 3);
    assert_eq!(view.total, 100);
    assert!(view.has_more, "total 100 > offset 5 + len 3");
    assert_eq!(view.next_page(), Some(Page::new(3, 5)));
}

// ---------- wire 参数派生（wiremock 锚定） ----------

fn client_for(base_url: &str) -> QqMusicClient {
    let config = ClientConfig {
        base_url: base_url.to_owned(),
        ..Default::default()
    };
    QqMusicClient::with_config(config)
}

fn req_matches(
    module: &'static str,
    method_name: &'static str,
    check: impl Fn(&serde_json::Value) -> bool + Send + Sync + 'static,
) -> impl wiremock::Match {
    move |req: &wiremock::Request| {
        let body: serde_json::Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let req_0 = &body["req_0"];
        req_0["module"] == json!(module)
            && req_0["method"] == json!(method_name)
            && check(&req_0["param"])
    }
}

fn ok_sub(data: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"code": 0, "req_0": {"code": 0, "data": data}}))
}

/// GetSingerListIndex：`sin = page.offset()`、`cur_page = page.page`
/// （服务端固定 80/页，Page::num 仅参与 offset 计算）。
#[tokio::test]
async fn singer_list_index_derives_sin_and_cur_page() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musichallSinger.SingerList",
            "GetSingerListIndex",
            |param| param["sin"] == json!(160) && param["cur_page"] == json!(3),
        ))
        .respond_with(ok_sub(json!({"singerlist": [], "total": 6803, "code": 0})))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_for(&server.uri());
    let api = SingerApi::new(&client);
    let resp = api
        .get_singer_list_index(
            hmp_qqmusic_api::singer::AreaType::All,
            hmp_qqmusic_api::singer::SexType::All,
            hmp_qqmusic_api::singer::GenreType::All,
            IndexType::All,
            Page::new(3, 80),
        )
        .await
        .unwrap();
    assert_eq!(resp.total, 6803);
}

/// CgiGetDiss（songlist.get_detail）：`song_begin = page.offset()`、
/// `song_num = page.num`（上游 num/page 双参数冗余归一为 Page）。
#[tokio::test]
async fn songlist_detail_derives_song_begin_and_song_num() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.srfDissInfo.DissInfo",
            "CgiGetDiss",
            |param| {
                param["song_begin"] == json!(100)
                    && param["song_num"] == json!(100)
                    && param["disstid"] == json!(9785418994i64)
            },
        ))
        .respond_with(ok_sub(json!({"total_song_num": 30, "songlist": [], "hasmore": 0})))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_for(&server.uri());
    let api = SonglistApi::new(&client);
    let resp = api
        .get_detail(9785418994i64, 0, Page::new(2, 100), false, true, true)
        .await
        .unwrap();
    assert_eq!(resp.total, 30);
}

/// GetHotCommentList：`PageNum = page.page - 1`、`PageSize = page.num`。
#[tokio::test]
async fn hot_comments_derive_page_num_zero_based() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.globalComment.CommentRead",
            "GetHotCommentList",
            |param| param["PageNum"] == json!(1) && param["PageSize"] == json!(20),
        ))
        .respond_with(ok_sub(json!({
            "CommentList": {"Comments": [], "HasMore": 0, "Total": 0},
            "TotalCmNum": 0,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_for(&server.uri());
    let api = CommentApi::new(&client);
    let resp = api.get_hot_comments(107192080, Page::new(2, 20)).await.unwrap();
    let view = resp.paged(Page::new(2, 20));
    assert!(view.items.is_empty());
    assert!(!view.has_more);
}

/// PlaylistFavRead：`offset = page.offset()`、`size = page.num`
/// （加密 uin 参数键 `uin`，2026-09-29 实测锚点沿用）。
#[tokio::test]
async fn fav_songlist_derives_offset_and_size() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/cgi-bin/musicu.fcg"))
        .and(req_matches(
            "music.musicasset.PlaylistFavRead",
            "CgiGetPlaylistFavInfo",
            |param| {
                param["uin"] == json!("NKoqNeC5NKSA")
                    && param["offset"] == json!(10)
                    && param["size"] == json!(10)
            },
        ))
        .respond_with(ok_sub(json!({"hasmore": 1, "v_list": [], "total": 12})))
        .expect(1)
        .mount(&server)
        .await;
    let cred = hmp_qqmusic_api::credential::Credential {
        uin: "939861972".into(),
        encrypt_uin: "NKoqNeC5NKSA".into(),
        ..Default::default()
    };
    let client = client_for(&server.uri());
    let api = UserApi::new(&client);
    let resp = api
        .get_fav_songlist("NKoqNeC5NKSA", Page::new(2, 10), Some(&cred))
        .await
        .unwrap();
    assert_eq!(resp.total, 12);
    assert!(resp.paged(Page::new(2, 10)).has_more);
}
