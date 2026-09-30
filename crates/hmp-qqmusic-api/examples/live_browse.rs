//! browse 域 live 探针（album/top/recommend/comment 真机核验 + 评论写闭环）。
//!
//! 运行：`cargo run -p hmp-qqmusic-api --example live_browse`
//! 补测（仅 fav 净零闭环 + 登录态推荐歌单封面核对 + withTags wire dump）：
//! `cargo run -p hmp-qqmusic-api --example live_browse -- fav`
//!
//! 覆盖：
//! - album：get_detail（MID/数字 ID）/ get_song / get_new_album（免登录）；
//! - top：get_category / get_detail（免登录，withTags=true 布尔参数）；
//! - recommend：get_home_feed / get_radar_recommend / get_recommend_songlist /
//!   get_recommend_newsong（免登录）+ get_guess_recommend（需登录，读）；
//! - comment：count / hot / new / recommend（免登录读）+ 写闭环：raw AddComment
//!   （wire dump + 手动提取 CmId）→ raw DelComment（wire dump + 删除）→
//!   公共 delete_comment 复删（上游语义「不存在也为 true」，兼作清理兜底）。
//!   全程仅创建 1 条评论且内容固定 "HMP API 测试评论，即将删除"；
//! - album fav_album/del_fav_album 净零闭环：先核对目标专辑**不在**用户收藏
//!   （get_fav_album 全量核对，不足全量则保守跳过；服务端单页上限 20），
//!   fav 成功才 del，净零变更。
//!
//! 安全：绝不调用 logout/refresh_credential；不碰既有收藏；请求间隔 ≥1.1s。

use hmp_qqmusic_api::album::{AlbumApi, AlbumFavWriteResponse};
use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::comment::{AddCommentResponse, CommentApi};
use hmp_qqmusic_api::credential::Credential;
use hmp_qqmusic_api::pagination::{Page, Paged};
use hmp_qqmusic_api::protocol::cgi::CgiRequest;
use hmp_qqmusic_api::recommend::RecommendApi;
use hmp_qqmusic_api::top::TopApi;
use hmp_qqmusic_api::user::UserApi;
use serde_json::{Value, json};

/// 兜底测试歌（孙燕姿《开始懂了》，专辑 1458791 首曲不可用时备用）。
const FALLBACK_SONG_ID: i64 = 186016;
/// 请求间隔。
const GAP: std::time::Duration = std::time::Duration::from_millis(1100);
/// 评论测试内容（固定文案，验证后立即删除）。
const TEST_COMMENT: &str = "HMP API 测试评论，即将删除";

fn load_credential() -> Option<Credential> {
    hmp_storage::credential::store_from_env()
        .load()
        .ok()
        .flatten()
        .filter(|c| c.is_logged_in())
}

async fn gap() {
    tokio::time::sleep(GAP).await;
}

/// 对象键列表（诊断输出用）。
fn keys(v: &Value) -> Vec<String> {
    v.as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// 子响应内层 data 的键。
fn data_keys(sub: &Value) -> Vec<String> {
    sub.get("data").map(keys).unwrap_or_default()
}

/// 复核：修复后的公共 API 评论闭环（add_comment 应取得真实 comment_id，
/// delete_comment 应返回 true）。全程 1 条评论，创建即删，净零变更。
async fn run_cmt_verify() {
    let client = QqMusicClient::new();
    let cmt = CommentApi::new(&client);
    let Some(cred) = load_credential() else {
        println!("credential: none（cmt 复核需登录）");
        return;
    };
    let biz_id = FALLBACK_SONG_ID;
    gap().await;
    match cmt.add_comment(biz_id, TEST_COMMENT, None, &cred).await {
        Ok(r) => {
            println!(
                "PASS comment.add_comment: comment_id={:?} subcode={} msg={:?} floor={}",
                r.comment_id, r.subcode, r.msg, r.floor
            );
            if r.comment_id.is_empty() {
                println!("FAIL comment.add_comment: comment_id 仍为空（别名未生效？）");
                return;
            }
            gap().await;
            match cmt.delete_comment(&r.comment_id, &cred).await {
                Ok(true) => println!("PASS comment.delete_comment: true（已删除，净零）"),
                other => println!(
                    "FAIL comment.delete_comment: {other:?}（请人工检查评论 {}）",
                    r.comment_id
                ),
            }
        }
        Err(e) => println!("FAIL comment.add_comment: {e}"),
    }
}

/// 补测：推荐歌单广场 wire dump（核对 List[*].Playlist.cover 的真实键形，
/// 登录/免登录下 `cover.default_url` 提取结果均为空，需确认服务端键形）。
async fn run_wire_dump() {
    let client = QqMusicClient::new();
    let cred = load_credential();
    gap().await;
    match client
        .musicu_request(
            &CgiRequest::new(
                "music.playlist.PlaylistSquare",
                "GetRecommendFeed",
                json!({"From": 0, "Size": 2}),
            ),
            cred.as_ref(),
        )
        .await
    {
        Ok(sub) => {
            let data = sub.get("data").cloned().unwrap_or(json!({}));
            println!("WIRE PlaylistSquare data keys={:?}", keys(&data));
            let first = data
                .get("List")
                .and_then(|l| l.get(0))
                .cloned()
                .unwrap_or(json!({}));
            println!("WIRE List[0] keys={:?}", keys(&first));
            let playlist = first.get("Playlist").cloned().unwrap_or(json!({}));
            println!("WIRE List[0].Playlist keys={:?}", keys(&playlist));
            println!(
                "WIRE List[0].Playlist.cover={}",
                playlist
                    .get("cover")
                    .map(|c| c.to_string())
                    .unwrap_or_default()
            );
            println!(
                "WIRE List[0].Playlist.creator={}",
                playlist
                    .get("creator")
                    .map(|c| c.to_string())
                    .unwrap_or_default()
            );
        }
        Err(e) => println!("WIRE PlaylistSquare FAIL: {e}"),
    }
}

/// 补测：仅 fav 净零闭环 + 登录态推荐歌单封面核对 + withTags wire dump。
///
/// 服务端 `CgiGetAlbumFavInfo` 单页上限 20（size=100 也只返回 20），
/// 按页扫全量后选一张未收藏新碟做 fav → del 净零闭环。
async fn run_fav_only() {
    let client = QqMusicClient::new();
    let album = AlbumApi::new(&client);
    let rec = RecommendApi::new(&client);
    let user = UserApi::new(&client);
    let Some(cred) = load_credential() else {
        println!("credential: none（fav 补测需登录）");
        return;
    };
    let euin = cred.encrypt_uin.clone();

    // 登录态下推荐歌单广场（免登录时 cover/creator 为空，验证是否登录增强）
    gap().await;
    match rec.get_recommend_songlist(Page::new(1, 5)).await {
        Ok(r) => println!(
            "NOTE get_recommend_songlist(登录态): lists={} first_cover={:?} first_nick={:?}",
            r.songlists.len(),
            r.songlists.first().map(|s| s.picurl.clone()),
            r.songlists.first().map(|s| s.creator_nick.clone())
        ),
        Err(e) => println!("FAIL get_recommend_songlist(登录态): {e}"),
    }

    // withTags=true wire dump（核对 songTagInfoList 是否下发）
    gap().await;
    match client
        .musicu_request(
            &CgiRequest::new(
                "music.musicToplist.Toplist",
                "GetDetail",
                json!({"topId": 62, "offset": 0, "num": 2, "withTags": true}),
            ),
            None,
        )
        .await
    {
        Ok(sub) => println!(
            "WIRE TopGetDetail(withTags=true) data keys={:?}",
            data_keys(&sub)
        ),
        Err(e) => println!("WIRE TopGetDetail FAIL: {e}"),
    }

    // 收藏专辑全量核对（单页 20）
    let mut fav_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut fav_total: i64 = 0;
    let mut covered = 0i64;
    let mut scan_ok = true;
    for page in 1..=3i64 {
        gap().await;
        match user
            .get_fav_album(&euin, Page::new(page as u32, 20), Some(&cred))
            .await
        {
            Ok(r) => {
                fav_total = r.total;
                covered += r.albums.len() as i64;
                fav_ids.extend(r.albums.iter().map(|a| a.id));
                if r.albums.len() < 20 || covered >= r.total {
                    break;
                }
            }
            Err(e) => {
                println!("FAIL user.get_fav_album(核对): {e} → 跳过 fav 闭环");
                scan_ok = false;
                break;
            }
        }
    }
    if !scan_ok {
        println!("SKIP album.fav_album: 收藏列表核对失败");
        return;
    }
    if covered < fav_total {
        println!("SKIP album.fav_album: 收藏 total={fav_total} 仅核对 {covered}，保守跳过");
        return;
    }
    println!("SCAN album fav: total={fav_total} covered={covered}");

    // 候选：新碟「其他」区最后一张（冷门）
    gap().await;
    let candidate = album
        .get_new_album(6, Page::new(1, 5))
        .await
        .ok()
        .and_then(|r| r.albums.last().map(|a| a.album.id).filter(|id| *id > 0))
        .filter(|id| !fav_ids.contains(id));
    let Some(album_id) = candidate else {
        println!("SKIP album.fav_album: 无合适候选（候选已在收藏或取碟失败）");
        return;
    };

    // 1) raw FavAlbum（wire dump + 真实收藏）
    gap().await;
    let fav_wire = client
        .musicu_request(
            &CgiRequest::new(
                "music.musicasset.AlbumFavWrite",
                "FavAlbum",
                json!({ "v_albumId": [album_id] }),
            )
            .with_require_login(true),
            Some(&cred),
        )
        .await;
    let mut favorited = false;
    match &fav_wire {
        Ok(sub) => {
            println!(
                "WIRE FavAlbum sub keys={:?} data keys={:?}",
                keys(sub),
                data_keys(sub)
            );
            println!("WIRE FavAlbum full={sub}");
            let parsed: AlbumFavWriteResponse =
                serde_json::from_value(sub.get("data").cloned().unwrap_or(json!({})))
                    .unwrap_or_default();
            println!(
                "PARSE FavAlbum: result={} failed={:?} success={}",
                parsed.result,
                parsed.failed_album_id,
                parsed.success()
            );
            favorited = parsed.success();
        }
        Err(e) => println!("FAIL WIRE FavAlbum(album_id={album_id}): {e}"),
    }
    if !favorited {
        println!("SKIP fav 闭环: FavAlbum 未成功（状态未变更，不执行 del）");
        return;
    }
    // 2) raw CancelFavAlbum（wire dump + 恢复状态）
    gap().await;
    match client
        .musicu_request(
            &CgiRequest::new(
                "music.musicasset.AlbumFavWrite",
                "CancelFavAlbum",
                json!({ "v_albumId": [album_id] }),
            )
            .with_require_login(true),
            Some(&cred),
        )
        .await
    {
        Ok(sub) => {
            println!(
                "WIRE CancelFavAlbum sub keys={:?} data keys={:?} full={sub}",
                keys(&sub),
                data_keys(&sub)
            );
            let parsed: AlbumFavWriteResponse =
                serde_json::from_value(sub.get("data").cloned().unwrap_or(json!({})))
                    .unwrap_or_default();
            println!(
                "PARSE CancelFavAlbum: result={} failed={:?} success={}",
                parsed.result,
                parsed.failed_album_id,
                parsed.success()
            );
        }
        Err(e) => println!("FAIL WIRE CancelFavAlbum: {e}（请人工检查 album_id={album_id}）"),
    }
    // 3) 公共 API 完整闭环：fav（再收藏）→ del（恢复，净零）
    gap().await;
    match album.fav_album(&[album_id], &cred).await {
        Ok(r) => println!(
            "PASS album.fav_album: result={} failed={:?} success={}",
            r.result,
            r.failed_album_id,
            r.success()
        ),
        Err(e) => println!("FAIL album.fav_album: {e}"),
    }
    gap().await;
    match album.del_fav_album(&[album_id], &cred).await {
        Ok(r) => println!(
            "PASS album.del_fav_album: result={} failed={:?} success={}（净零恢复）",
            r.result,
            r.failed_album_id,
            r.success()
        ),
        Err(e) => {
            println!("FAIL album.del_fav_album: {e}（请人工检查 album_id={album_id} 收藏状态）")
        }
    }
}

#[tokio::main]
async fn main() {
    if std::env::args().any(|a| a == "fav") {
        run_fav_only().await;
        return;
    }
    if std::env::args().any(|a| a == "wire") {
        run_wire_dump().await;
        return;
    }
    if std::env::args().any(|a| a == "cmt") {
        run_cmt_verify().await;
        return;
    }
    let client = QqMusicClient::new();
    let album = AlbumApi::new(&client);
    let top = TopApi::new(&client);
    let rec = RecommendApi::new(&client);
    let cmt = CommentApi::new(&client);
    let user = UserApi::new(&client);
    let cred = load_credential();

    match &cred {
        Some(c) => println!("credential: uin={} euin_len={}", c.uin, c.encrypt_uin.len()),
        None => println!("credential: none (login-required APIs will SKIP)"),
    }

    // ---------- Phase A：album（免登录） ----------
    gap().await;
    match album.get_detail("003RMaRI1iFoYd").await {
        Ok(r) => println!(
            "PASS album.get_detail(mid): name={:?} time={:?} company={:?} singers={} lang={:?}",
            r.album.album.name,
            r.album.album.time_public,
            r.company.name,
            r.singers.len(),
            r.album.language
        ),
        Err(e) => println!("FAIL album.get_detail(mid): {e}"),
    }

    gap().await;
    match album.get_detail("1458791").await {
        Ok(r) => println!(
            "PASS album.get_detail(numeric albumId): id={} name={:?} desc_len={}",
            r.album.album.id,
            r.album.album.name,
            r.album.desc.len()
        ),
        Err(e) => println!("FAIL album.get_detail(numeric): {e}"),
    }

    gap().await;
    let mut first_song_id: i64 = FALLBACK_SONG_ID;
    match album.get_song("1458791", Page::new(1, 5)).await {
        Ok(r) => {
            if let Some(s) = r.song_list.first() {
                if s.id > 0 {
                    first_song_id = s.id;
                }
            }
            println!(
                "PASS album.get_song: album_mid={:?} total={} parsed={} first_id={first_song_id}",
                r.album_mid,
                r.total_num,
                r.song_list.len()
            );
        }
        Err(e) => println!("FAIL album.get_song: {e}（用兜底 song id={FALLBACK_SONG_ID}）"),
    }

    gap().await;
    // fav 闭环候选：新碟「其他」区最后一张（冷门，降低已收藏概率）
    let mut fav_candidate: Option<i64> = None;
    match album.get_new_album(6, Page::new(1, 5)).await {
        Ok(r) => {
            fav_candidate = r.albums.last().map(|a| a.album.id).filter(|id| *id > 0);
            println!(
                "PASS album.get_new_album(area=6): total={} parsed={} first={:?} candidate={fav_candidate:?}",
                r.total,
                r.albums.len(),
                r.albums.first().map(|a| a.album.name.clone())
            );
        }
        Err(e) => println!("FAIL album.get_new_album: {e}"),
    }

    // ---------- Phase B：top（免登录） ----------
    gap().await;
    match top.get_category().await {
        Ok(r) => println!(
            "PASS top.get_category: groups={} first_group={:?} first_top={:?} preview_songs={}",
            r.group.len(),
            r.group.first().map(|g| g.name.clone()),
            r.group
                .first()
                .and_then(|g| g.toplist.first().map(|t| t.name.clone())),
            r.group
                .first()
                .and_then(|g| g.toplist.first().map(|t| t.songs.len()))
                .unwrap_or(0)
        ),
        Err(e) => println!("FAIL top.get_category: {e}"),
    }

    gap().await;
    match top.get_detail(62, Page::new(1, 5), true).await {
        Ok(r) => println!(
            "PASS top.get_detail(id=62, withTags=true): name={:?} total={} songs={} tags={} ext={}",
            r.info.name,
            r.info.total_num,
            r.songs.len(),
            r.song_tags.len(),
            r.ext_info_list.len()
        ),
        Err(e) => println!("FAIL top.get_detail: {e}"),
    }

    // ---------- Phase C：recommend（免登录） ----------
    gap().await;
    match rec.get_home_feed(1, 0, 0, &[]).await {
        Ok(r) => println!(
            "PASS recommend.get_home_feed: retcode={} shelves={} first_niches={}",
            r.retcode,
            r.shelves.len(),
            r.shelves.first().map(|s| s.niches.len()).unwrap_or(0)
        ),
        Err(e) => println!("FAIL recommend.get_home_feed: {e}"),
    }

    gap().await;
    match rec.get_radar_recommend(1).await {
        Ok(r) => println!(
            "PASS recommend.get_radar_recommend: songs={} ids={} has_more={} toast={:?}",
            r.songs.len(),
            r.recommend_song_ids.len(),
            r.has_more,
            r.toast
        ),
        Err(e) => println!("FAIL recommend.get_radar_recommend: {e}"),
    }

    gap().await;
    match rec.get_recommend_songlist(Page::new(1, 5)).await {
        Ok(r) => println!(
            "PASS recommend.get_recommend_songlist: lists={} has_more={} from_limit={} first={:?} cover={:?} nick={:?}",
            r.songlists.len(),
            r.has_more,
            r.from_limit,
            r.songlists.first().map(|s| s.title.clone()),
            r.songlists.first().map(|s| s.picurl.clone()),
            r.songlists.first().map(|s| s.creator_nick.clone())
        ),
        Err(e) => println!("FAIL recommend.get_recommend_songlist: {e}"),
    }

    gap().await;
    match rec.get_recommend_newsong(5).await {
        Ok(r) => println!(
            "PASS recommend.get_recommend_newsong(type=5): songs={} lan={:?} tags={}",
            r.songs.len(),
            r.lan,
            r.song_tags.len()
        ),
        Err(e) => println!("FAIL recommend.get_recommend_newsong: {e}"),
    }

    // ---------- Phase D：comment 读（免登录） ----------
    let biz_id = first_song_id;
    gap().await;
    match cmt.get_comment_count(biz_id).await {
        Ok(n) => println!("PASS comment.get_comment_count(biz_id={biz_id}): count={n}"),
        Err(e) => println!("FAIL comment.get_comment_count: {e}"),
    }

    gap().await;
    match cmt.get_hot_comments(biz_id, Page::new(1, 3)).await {
        Ok(resp) => {
            let view = resp.paged(Page::new(1, 3));
            println!(
                "PASS comment.get_hot_comments: n={} first_nick={:?} first_cm_id={:?} praise={} total={} has_more={}",
                view.items.len(),
                view.items.first().map(|c| c.nickname.clone()),
                view.items.first().map(|c| c.cm_id.clone()),
                view.items.first().map(|c| c.like_count).unwrap_or(0),
                view.total,
                view.has_more
            );
        }
        Err(e) => println!("FAIL comment.get_hot_comments: {e}"),
    }

    gap().await;
    match cmt.get_new_comments(biz_id, Page::new(1, 3)).await {
        Ok(resp) => {
            let view = resp.paged(Page::new(1, 3));
            println!(
                "PASS comment.get_new_comments: n={} first_content_len={} has_more={}",
                view.items.len(),
                view.items.first().map(|c| c.content.len()).unwrap_or(0),
                view.has_more
            );
        }
        Err(e) => println!("FAIL comment.get_new_comments: {e}"),
    }

    gap().await;
    match cmt.get_recommend_comments(biz_id, Page::new(1, 3)).await {
        Ok(resp) => {
            let view = resp.paged(Page::new(1, 3));
            println!(
                "PASS comment.get_recommend_comments: n={} first_nick={:?} has_more={}",
                view.items.len(),
                view.items.first().map(|c| c.nickname.clone()),
                view.has_more
            );
        }
        Err(e) => println!("FAIL comment.get_recommend_comments: {e}"),
    }

    // ---------- 后续阶段需登录 ----------
    let Some(cred) = cred else {
        println!("SKIP guess/写闭环: 未登录");
        println!("done (partial)");
        return;
    };

    // ---------- Phase E：recommend 猜你喜欢（需登录，读）+ wire dump ----------
    gap().await;
    match rec.get_guess_recommend(&cred).await {
        Ok(r) => println!(
            "PASS recommend.get_guess_recommend: songs={} first={:?}",
            r.songs.len(),
            r.songs.first().map(|s| s.name.clone())
        ),
        Err(e) => println!("FAIL recommend.get_guess_recommend: {e}"),
    }
    // wire dump：核对 data 键（上游别名 tracks）
    gap().await;
    match client
        .musicu_request(
            &CgiRequest::new(
                "music.radioProxy.MbTrackRadioSvr",
                "get_radio_track",
                json!({"id": 99, "num": 5, "from": 0, "scene": 0, "song_ids": []}),
            ),
            Some(&cred),
        )
        .await
    {
        Ok(sub) => {
            println!("WIRE guess data keys={:?}", data_keys(&sub));
            let tracks = sub
                .get("data")
                .and_then(|d| d.get("tracks"))
                .and_then(|t| t.get(0))
                .map(keys)
                .unwrap_or_default();
            println!("WIRE guess tracks[0] keys={tracks:?}");
        }
        Err(e) => println!("WIRE guess FAIL: {e}"),
    }

    // ---------- Phase F：comment 写闭环（全程仅 1 条评论，创建即删） ----------
    gap().await;
    let mut added_cm_id: Option<String> = None;
    match client
        .musicu_request(
            &CgiRequest::new(
                "music.globalComment.CommentWriteServer",
                "AddComment",
                json!({
                    "Content": TEST_COMMENT,
                    "BizType": 1,
                    "BizId": biz_id.to_string(),
                    "BizSubType": 2,
                }),
            )
            .with_require_login(true),
            Some(&cred),
        )
        .await
    {
        Ok(sub) => {
            println!(
                "WIRE AddComment sub keys={:?} data keys={:?}",
                keys(&sub),
                data_keys(&sub)
            );
            println!("WIRE AddComment full={sub}");
            // 复刻公共 API 解析路径，验证 AddCommentResponse 字段别名
            let parsed: Result<AddCommentResponse, _> =
                serde_json::from_value(sub.get("data").cloned().unwrap_or(json!({})));
            if let Ok(p) = parsed {
                println!(
                    "PARSE AddCommentResponse: comment_id={:?} ret={}（若 comment_id 为空即别名缺陷）",
                    p.comment_id, p.ret
                );
            }
            added_cm_id = sub
                .get("data")
                .and_then(|d| {
                    d.get("AddedCmId")
                        .or_else(|| d.get("commentId"))
                        .or_else(|| d.get("CmId"))
                })
                .and_then(|v| v.as_str())
                .map(str::to_owned);
        }
        Err(e) => println!("FAIL WIRE AddComment: {e}"),
    }

    if let Some(cm_id) = added_cm_id.clone() {
        // 立即删除（自清理）
        gap().await;
        match client
            .musicu_request(
                &CgiRequest::new(
                    "music.globalComment.CommentWriteServer",
                    "DelComment",
                    json!({ "CommentId": cm_id }),
                )
                .with_require_login(true),
                Some(&cred),
            )
            .await
        {
            Ok(sub) => println!(
                "WIRE DelComment sub keys={:?} data keys={:?} full={sub}",
                keys(&sub),
                data_keys(&sub)
            ),
            Err(e) => println!("FAIL WIRE DelComment: {e}（评论 cm_id={cm_id} 待清理）"),
        }
        // 公共 API 复删（上游语义：评论不存在也返回 true；兼作清理兜底）
        gap().await;
        match cmt.delete_comment(&cm_id, &cred).await {
            Ok(v) => println!("PASS comment.delete_comment(评论已不存在): {v}（上游语义 true）"),
            Err(e) => println!("FAIL comment.delete_comment: {e}"),
        }
    } else {
        println!("SKIP comment.delete_comment: add 未取得 cm_id（无需清理）");
    }

    // ---------- Phase G：album fav_album/del_fav_album 净零闭环 ----------
    // 前置：用户收藏专辑全量核对（目标不在收藏中才执行；fav 失败则不 del）。
    let euin = cred.encrypt_uin.clone();
    gap().await;
    let mut fav_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut fav_total: i64 = 0;
    let mut covered = 0i64;
    let mut scan_ok = true;
    for page in 1..=3i64 {
        match user
            .get_fav_album(&euin, Page::new(page as u32, 100), Some(&cred))
            .await
        {
            Ok(r) => {
                fav_total = r.total;
                covered += r.albums.len() as i64;
                fav_ids.extend(r.albums.iter().map(|a| a.id));
                if r.albums.len() < 100 || covered >= r.total {
                    break;
                }
            }
            Err(e) => {
                println!("FAIL user.get_fav_album(核对): {e} → 跳过 fav 闭环");
                scan_ok = false;
                break;
            }
        }
        if page < 3 {
            gap().await;
        }
    }

    if !scan_ok {
        println!("SKIP album.fav_album: 收藏列表核对失败");
    } else if covered < fav_total {
        println!("SKIP album.fav_album: 收藏 total={fav_total} 仅核对 {covered}，保守跳过");
    } else {
        println!("SCAN album fav: total={fav_total} covered={covered}");
        let candidate = fav_candidate.filter(|id| !fav_ids.contains(id));
        match candidate {
            None => println!("SKIP album.fav_album: 无合适候选（候选已在收藏或取碟失败）"),
            Some(album_id) => {
                // 1) raw FavAlbum（wire dump + 真实收藏）
                gap().await;
                let fav_wire = client
                    .musicu_request(
                        &CgiRequest::new(
                            "music.musicasset.AlbumFavWrite",
                            "FavAlbum",
                            json!({ "v_albumId": [album_id] }),
                        )
                        .with_require_login(true),
                        Some(&cred),
                    )
                    .await;
                let mut favorited = false;
                match &fav_wire {
                    Ok(sub) => {
                        println!(
                            "WIRE FavAlbum sub keys={:?} data keys={:?} full={sub}",
                            keys(sub),
                            data_keys(sub)
                        );
                        // 复刻公共 API 解析路径
                        let parsed: AlbumFavWriteResponse =
                            serde_json::from_value(sub.get("data").cloned().unwrap_or(json!({})))
                                .unwrap_or_default();
                        println!(
                            "PARSE FavAlbum: result={} failed={:?} success={}",
                            parsed.result,
                            parsed.failed_album_id,
                            parsed.success()
                        );
                        favorited = parsed.success();
                    }
                    Err(e) => println!("FAIL WIRE FavAlbum(album_id={album_id}): {e}"),
                }
                if favorited {
                    // 2) raw CancelFavAlbum（wire dump + 恢复状态）
                    gap().await;
                    match client
                        .musicu_request(
                            &CgiRequest::new(
                                "music.musicasset.AlbumFavWrite",
                                "CancelFavAlbum",
                                json!({ "v_albumId": [album_id] }),
                            )
                            .with_require_login(true),
                            Some(&cred),
                        )
                        .await
                    {
                        Ok(sub) => {
                            println!(
                                "WIRE CancelFavAlbum sub keys={:?} data keys={:?} full={sub}",
                                keys(&sub),
                                data_keys(&sub)
                            );
                            let parsed: AlbumFavWriteResponse = serde_json::from_value(
                                sub.get("data").cloned().unwrap_or(json!({})),
                            )
                            .unwrap_or_default();
                            println!(
                                "PARSE CancelFavAlbum: result={} failed={:?} success={}",
                                parsed.result,
                                parsed.failed_album_id,
                                parsed.success()
                            );
                        }
                        Err(e) => println!(
                            "FAIL WIRE CancelFavAlbum: {e}（请人工检查 album_id={album_id}）"
                        ),
                    }
                    // 3) 公共 API 完整闭环：fav（再收藏）→ del（恢复，净零）
                    gap().await;
                    match album.fav_album(&[album_id], &cred).await {
                        Ok(r) => println!(
                            "PASS album.fav_album: result={} failed={:?} success={}",
                            r.result,
                            r.failed_album_id,
                            r.success()
                        ),
                        Err(e) => println!("FAIL album.fav_album: {e}"),
                    }
                    gap().await;
                    match album.del_fav_album(&[album_id], &cred).await {
                        Ok(r) => println!(
                            "PASS album.del_fav_album: result={} failed={:?} success={}（净零恢复）",
                            r.result,
                            r.failed_album_id,
                            r.success()
                        ),
                        Err(e) => println!(
                            "FAIL album.del_fav_album: {e}（请人工检查 album_id={album_id} 收藏状态）"
                        ),
                    }
                } else {
                    println!(
                        "SKIP album.fav_album 闭环: FavAlbum 未成功（状态未变更，不执行 del）"
                    );
                }
            }
        }
    }

    println!("done");
}
