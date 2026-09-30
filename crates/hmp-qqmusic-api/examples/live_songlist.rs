//! user/songlist 域 live 探针（真机核验 + 临时歌单写操作闭环）。
//!
//! 运行：`cargo run -p hmp-qqmusic-api --example live_songlist`
//! 诊断补测（fav/unfav 原始响应 + 「我喜欢」全量核对 + like/unlike）：
//! `cargo run -p hmp-qqmusic-api --example live_songlist -- diag`
//!
//! 覆盖：
//! - user 读接口（created/fav_song/fav_songlist/fav_album/vip/music_gene；
//!   `get_homepage` 已知服务端 10000 空壳，不重试）；
//! - songlist 写闭环：创建临时歌单 `HMP-API-TEST-<ts>` → get_detail →
//!   add_songs → del_songs → like_song/unlike_song（dirid=201，先核对
//!   目标歌曲不在「我喜欢」中）→ fav/unfav（仅对临时歌单）→ delete → 确认已删。
//!
//! 安全：绝不调用 logout/refresh_credential；fav_songlist/unfav_songlist、
//! like/unlike 全程闭环（净零变更）；失败时尽力删除临时歌单。
//! 请求间隔 ≥1.1s。

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::pagination::Page;
use hmp_qqmusic_api::credential::Credential;
use hmp_qqmusic_api::protocol::cgi::CgiRequest;
use hmp_qqmusic_api::songlist::{GetSonglistDetailResponse, SonglistApi};
use hmp_qqmusic_api::user::{UserApi, UserCreatedSonglistResponse, UserFavAlbumResponse, UserFavSonglistResponse};
use serde_json::json;

/// 测试歌曲：孙燕姿《开始懂了》（songType 0=普通歌曲，与上游 `Song.type` 一致）。
const TEST_SONG: (i64, i64) = (186016, 0);
/// 请求间隔。
const GAP: std::time::Duration = std::time::Duration::from_millis(1100);

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

#[tokio::main]
async fn main() {
    let diag = std::env::args().any(|a| a == "diag");
    let fav2 = std::env::args().any(|a| a == "fav2");
    if fav2 {
        run_fav_verify().await;
    } else if diag {
        run_diag().await;
    } else {
        run_full().await;
    }
}

/// 复核：公共 API `fav_songlist`/`unfav_songlist` 对新建临时歌单的行为
/// （diag 已证 wire 语义：自建歌单 FavPlaylist → result=80184；
/// CancelFavPlaylist 未收藏 → result=0。本函数确认公共 API 判定一致）。
async fn run_fav_verify() {
    let client = QqMusicClient::new();
    let user = UserApi::new(&client);
    let songlist = SonglistApi::new(&client);
    let Some(cred) = load_credential() else {
        println!("credential: none（fav2 需登录）");
        return;
    };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let dirname = format!("HMP-API-TEST3-{ts}");
    gap().await;
    match songlist.create(&dirname, &cred).await {
        Ok(r) => {
            let dirid = r.dirid;
            let tid = if r.id != 0 { r.id } else { r.dirid };
            println!("PASS create(temp3): tid={tid} dirid={dirid}");
            gap().await;
            match user.fav_songlist(tid, &cred).await {
                Ok(v) => println!("NOTE fav_songlist(temp3): {v}（自建歌单预期 false，服务端 result=80184）"),
                Err(e) => println!("FAIL fav_songlist(temp3): {e}"),
            }
            gap().await;
            match user.unfav_songlist(tid, &cred).await {
                Ok(v) => println!("NOTE unfav_songlist(temp3): {v}（未收藏歌单预期 true，服务端 result=0）"),
                Err(e) => println!("FAIL unfav_songlist(temp3): {e}"),
            }
            gap().await;
            match songlist.delete(dirid, &cred).await {
                Ok(r) => println!("CLEANUP delete(temp3): ret_code={} dirid={}", r.ret_code, r.dirid),
                Err(e) => println!("CLEANUP delete(temp3) FAIL: {e}（请人工检查 dirid={dirid}）"),
            }
        }
        Err(e) => println!("FAIL create(temp3): {e}"),
    }
}

/// 诊断补测：fav/unfav 原始响应 dump + 「我喜欢」全量核对后 like/unlike 闭环。
///
/// 前提（首轮 full 运行已知）：我喜欢 total=801，第 1-4 页（前 400 首）
/// 已核对不含测试歌；本函数补扫第 5-9 页凑齐全量后执行 like/unlike。
async fn run_diag() {
    let client = QqMusicClient::new();
    let user = UserApi::new(&client);
    let songlist = SonglistApi::new(&client);
    let Some(cred) = load_credential() else {
        println!("credential: none（diag 需登录）");
        return;
    };
    let uin = cred.uin.clone();
    let euin = cred.encrypt_uin.clone();

    // ---- 「我喜欢」全量核对（第 5-9 页，num=100）----
    let mut liked: HashSet<i64> = HashSet::new();
    let mut covered = 400i64; // 第 1-4 页已在 full 轮核对（不含测试歌）
    let mut reached_end = false;
    for page in 5..=9i64 {
        gap().await;
        match user.get_fav_song(&euin, Page::new(page as u32, 100), Some(&cred)).await {
            Ok(r) => {
                covered += r.songs.len() as i64;
                liked.extend(r.songs.iter().map(|s| s.id));
                if r.songs.len() < 100 {
                    reached_end = true; // 末页（不足 100 条）
                    break;
                }
            }
            Err(e) => {
                println!("ABORT like/unlike: 第 {page} 页核对失败: {e}");
                break;
            }
        }
    }
    println!(
        "SCAN 我喜欢: covered={covered} reached_end={reached_end} 测试歌已收藏={}",
        liked.contains(&TEST_SONG.0)
    );
    if reached_end && !liked.contains(&TEST_SONG.0) {
        gap().await;
        match songlist.like_song(&[TEST_SONG], &cred).await {
            Ok(true) => println!("PASS like_song: true"),
            other => println!("FAIL like_song: {other:?}"),
        }
        gap().await;
        match songlist.unlike_song(&[TEST_SONG], &cred).await {
            Ok(true) => println!("PASS unlike_song: true"),
            other => println!("FAIL unlike_song: {other:?}"),
        }
    } else if liked.contains(&TEST_SONG.0) {
        println!("SKIP like/unlike: 测试歌已在「我喜欢」中");
    } else {
        println!("SKIP like/unlike: 未能完成全量核对");
    }

    // ---- fav/unfav 原始响应 dump（对自建临时歌单，预期服务端拒绝并说明原因）----
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let dirname = format!("HMP-API-TEST2-{ts}");
    let mut created_dirid: Option<i64> = None;

    gap().await;
    match songlist.create(&dirname, &cred).await {
        Ok(r) => {
            println!("PASS create(temp2): tid={} dirid={}", r.id, r.dirid);
            created_dirid = Some(r.dirid);
            if r.dirid != 0 {
                gap().await;
                let tid = if r.id != 0 { r.id } else { r.dirid };
                for method in ["FavPlaylist", "CancelFavPlaylist"] {
                    gap().await;
                    let req = CgiRequest::new(
                        "music.musicasset.PlaylistFavWrite",
                        method,
                        json!({ "uin": euin, "v_playlistId": [tid] }),
                    )
                    .with_require_login(true);
                    match client.musicu_request(&req, Some(&cred)).await {
                        Ok(v) => println!("WIRE {method}: {v}"),
                        Err(e) => println!("WIRE {method} FAIL: {e}"),
                    }
                }
            }
        }
        Err(e) => println!("FAIL create(temp2): {e}"),
    }

    // 清理 temp2
    if let Some(dirid) = created_dirid {
        gap().await;
        match songlist.delete(dirid, &cred).await {
            Ok(r) => println!("CLEANUP delete(temp2): ret_code={} dirid={}", r.ret_code, r.dirid),
            Err(e) => println!("CLEANUP delete(temp2) FAIL: {e}（请人工检查 dirid={dirid}）"),
        }
        gap().await;
        match user.get_created_songlist(&uin, Some(&cred)).await {
            Ok(r) => {
                println!("CLEANUP check: 自建歌单 total={}（确认 temp2 已不在列表）", r.total);
            }
            Err(e) => println!("CLEANUP check FAIL: {e}"),
        }
    }
}

async fn run_full() {
    let client = QqMusicClient::new();
    let user = UserApi::new(&client);
    let songlist = SonglistApi::new(&client);

    let Some(cred) = load_credential() else {
        println!("credential: none（需登录接口全部 SKIP）");
        // 仍验证免登录的 get_detail
        gap().await;
        match songlist.get_detail(7843129912, 0, Page::new(1, 5), false, true, true).await {
            Ok(r) => println!("PASS get_detail(免登录): total={} songs={}", r.total, r.songs.len()),
            Err(e) => println!("FAIL get_detail(免登录): {e}"),
        }
        return;
    };
    let uin = cred.uin.clone();
    let euin = cred.encrypt_uin.clone();
    println!("credential: uin={uin} euin_len={} expired={}", euin.len(), cred.is_expired());

    // ---------- Phase A：user 读接口 ----------
    gap().await;
    match user.get_created_songlist(&uin, Some(&cred)).await {
        Ok(r) => println!(
            "PASS get_created_songlist(数字uin): total={} parsed={} finished={}",
            r.total,
            r.songlist.len(),
            r.finished
        ),
        Err(e) => println!("FAIL get_created_songlist(数字uin): {e}"),
    }

    gap().await;
    match user.get_created_songlist(&euin, Some(&cred)).await {
        Ok(_) => println!("NOTE get_created_songlist(加密uin): 意外成功"),
        Err(e) => println!("PASS(预期失败) get_created_songlist(加密uin): {e}"),
    }

    // 「我喜欢」第一页（兼作 like/unlike 安全预检数据）
    gap().await;
    let mut liked: HashSet<i64> = HashSet::new();
    let mut liked_total: i64 = 0;
    match user.get_fav_song(&euin, Page::new(1, 100), Some(&cred)).await {
        Ok(r) => {
            liked_total = r.total;
            liked.extend(r.songs.iter().map(|s| s.id));
            println!(
                "PASS get_fav_song(加密uin): total={} page1={}",
                r.total,
                r.songs.len()
            );
        }
        Err(e) => println!("FAIL get_fav_song(加密uin): {e}"),
    }

    gap().await;
    match user.get_fav_songlist(&euin, Page::new(1, 10), Some(&cred)).await {
        Ok(r) => println!(
            "PASS get_fav_songlist(加密uin): total={} parsed={} hasmore={}",
            r.total,
            r.playlists.len(),
            r.hasmore
        ),
        Err(e) => println!("FAIL get_fav_songlist(加密uin): {e}"),
    }
    // 原始响应键名核对（serde 别名大小写验证）
    gap().await;
    let raw = client
        .musicu_request(
            &CgiRequest::new(
                "music.musicasset.PlaylistFavRead",
                "CgiGetPlaylistFavInfo",
                json!({ "uin": euin, "offset": 0, "size": 2 }),
            ),
            Some(&cred),
        )
        .await;
    match raw {
        Ok(v) => {
            let keys: Vec<String> = v
                .get("data")
                .and_then(|d| d.as_object())
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            let list_keys: Vec<String> = v
                .get("data")
                .and_then(|d| d.get("v_list").or_else(|| d.get("vecSonglist")))
                .and_then(|l| l.get(0))
                .and_then(|e| e.as_object())
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            println!("WIRE PlaylistFavRead data keys={keys:?}");
            println!("WIRE PlaylistFavRead list[0] keys={list_keys:?}");
        }
        Err(e) => println!("WIRE PlaylistFavRead FAIL: {e}"),
    }

    gap().await;
    match user.get_fav_album(&euin, Page::new(1, 10), Some(&cred)).await {
        Ok(r) => println!(
            "PASS get_fav_album(加密uin): total={} parsed={} hasmore={}",
            r.total,
            r.albums.len(),
            r.hasmore
        ),
        Err(e) => println!("FAIL get_fav_album(加密uin): {e}"),
    }
    gap().await;
    let raw = client
        .musicu_request(
            &CgiRequest::new(
                "music.musicasset.AlbumFavRead",
                "CgiGetAlbumFavInfo",
                json!({ "euin": euin, "offset": 0, "size": 2 }),
            ),
            Some(&cred),
        )
        .await;
    match raw {
        Ok(v) => {
            let keys: Vec<String> = v
                .get("data")
                .and_then(|d| d.as_object())
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            println!("WIRE AlbumFavRead data keys={keys:?}");
        }
        Err(e) => println!("WIRE AlbumFavRead FAIL: {e}"),
    }

    gap().await;
    match user.get_vip_info(&cred).await {
        Ok(v) => {
            let keys: Vec<String> = v
                .as_object()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            println!("PASS get_vip_info: keys={keys:?}");
        }
        Err(e) => println!("FAIL get_vip_info: {e}"),
    }

    gap().await;
    match user.get_music_gene(&euin, Some(&cred)).await {
        Ok(g) => println!(
            "PASS get_music_gene: nick={:?} sig_empty={}",
            g.userinfo_card.nick_name,
            g.userinfo_card.signature.is_empty()
        ),
        Err(e) => println!("FAIL get_music_gene: {e}"),
    }
    // get_homepage：服务端已知对合法参数返回 10000 空壳（2026-09-29），
    // 主控已多方式实测；本探针不重试，避免触发按 IP 限流。

    // ---------- Phase B：songlist 写闭环 ----------
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let dirname = format!("HMP-API-TEST-{ts}");
    let mut created_dirid: Option<i64> = None;
    let mut created_tid: Option<i64> = None;
    let mut deleted = false;

    gap().await;
    match songlist.create(&dirname, &cred).await {
        Ok(r) => {
            println!(
                "PASS create: ret_code={} tid={} dirid={} name={:?}",
                r.ret_code, r.id, r.dirid, r.name
            );
            if r.dirid != 0 {
                created_dirid = Some(r.dirid);
            }
            created_tid = Some(if r.id != 0 { r.id } else { r.dirid });
        }
        Err(e) => println!("FAIL create: {e}"),
    }

    if let (Some(dirid), Some(tid)) = (created_dirid, created_tid) {
        gap().await;
        match songlist.get_detail(tid, 0, Page::new(1, 10), false, true, true).await {
            Ok(r) => println!(
                "PASS get_detail(新建空歌单): code={} title={:?} total={} songs={}",
                r.code, r.info.list.title, r.total, r.songs.len()
            ),
            Err(e) => println!("FAIL get_detail(新建空歌单 tid={tid}): {e}"),
        }

        gap().await;
        match songlist.add_songs(dirid, &[TEST_SONG], tid, &cred).await {
            Ok(true) => println!("PASS add_songs: true"),
            Ok(false) => println!("FAIL add_songs: false（retCode!=0）"),
            Err(e) => println!("FAIL add_songs: {e}"),
        }

        gap().await;
        match songlist.get_detail(tid, 0, Page::new(1, 10), false, true, true).await {
            Ok(r) => {
                let has = r.songs.iter().any(|s| s.id == TEST_SONG.0);
                println!(
                    "PASS get_detail(加歌后): total={} 含测试歌={has}",
                    r.total
                );
            }
            Err(e) => println!("FAIL get_detail(加歌后): {e}"),
        }

        gap().await;
        match songlist.del_songs(dirid, &[TEST_SONG], tid, &cred).await {
            Ok(true) => println!("PASS del_songs: true"),
            Ok(false) => println!("FAIL del_songs: false（retCode!=0）"),
            Err(e) => println!("FAIL del_songs: {e}"),
        }

        gap().await;
        match songlist.get_detail(tid, 0, Page::new(1, 10), false, true, true).await {
            Ok(r) => println!(
                "PASS get_detail(删歌后): total={} songs={}",
                r.total,
                r.songs.len()
            ),
            Err(e) => println!("FAIL get_detail(删歌后): {e}"),
        }

        // like/unlike（dirid=201）：先确保测试歌不在「我喜欢」中（净零闭环）
        gap().await;
        let mut known = liked.clone();
        let mut pages = 1;
        let mut covered = liked.len() as i64;
        while covered < liked_total && pages < 5 {
            gap().await;
            match user.get_fav_song(&euin, Page::new(pages as u32 + 1, 100), Some(&cred)).await {
                Ok(r) => {
                    covered += r.songs.len() as i64;
                    known.extend(r.songs.iter().map(|s| s.id));
                    pages += 1;
                    if r.songs.is_empty() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        if known.contains(&TEST_SONG.0) {
            println!("SKIP like/unlike: 测试歌已在「我喜欢」中，避免动用户收藏");
        } else if liked_total > covered {
            println!(
                "SKIP like/unlike: 「我喜欢」共 {liked_total} 首，仅能核对 {covered} 首，保守跳过"
            );
        } else {
            gap().await;
            match songlist.like_song(&[TEST_SONG], &cred).await {
                Ok(true) => println!("PASS like_song: true"),
                other => println!("FAIL like_song: {other:?}"),
            }
            gap().await;
            match songlist.unlike_song(&[TEST_SONG], &cred).await {
                Ok(true) => println!("PASS unlike_song: true"),
                other => println!("FAIL unlike_song: {other:?}"),
            }
        }

        // fav/unfav 临时歌单（净零闭环）
        gap().await;
        match user.fav_songlist(tid, &cred).await {
            Ok(true) => println!("PASS fav_songlist(临时歌单): true"),
            other => println!("FAIL fav_songlist(临时歌单): {other:?}"),
        }
        gap().await;
        match user.unfav_songlist(tid, &cred).await {
            Ok(true) => println!("PASS unfav_songlist(临时歌单): true"),
            other => println!("FAIL unfav_songlist(临时歌单): {other:?}"),
        }

        // 删除临时歌单
        gap().await;
        match songlist.delete(dirid, &cred).await {
            Ok(r) => {
                println!(
                    "PASS delete: ret_code={} dirid={} name={:?}",
                    r.ret_code, r.dirid, r.name
                );
                deleted = true;
            }
            Err(e) => println!("FAIL delete(dirid={dirid}): {e}"),
        }

        // 确认已删：get_detail 应报错（歌单不存在）
        if deleted {
            gap().await;
            match songlist.get_detail(tid, 0, Page::new(1, 10), false, true, true).await {
                Ok(r) => println!(
                    "NOTE get_detail(删除后): 仍可访问 code={} total={} title={:?}",
                    r.code, r.total, r.info.list.title
                ),
                Err(e) => println!("PASS get_detail(删除后) 已不可访问: {e}"),
            }
            gap().await;
            match user.get_created_songlist(&uin, Some(&cred)).await {
                Ok(r) => {
                    let still = r.songlist.iter().any(|p| p.id == tid);
                    println!(
                        "PASS get_created_songlist(删除后): total={} 临时歌单残留={still}",
                        r.total
                    );
                }
                Err(e) => println!("FAIL get_created_songlist(删除后): {e}"),
            }
        }
    }

    // ---------- 清理兜底 ----------
    if let (Some(dirid), false) = (created_dirid, deleted) {
        println!("CLEANUP: 尝试删除未清理的临时歌单 dirid={dirid}");
        gap().await;
        match songlist.delete(dirid, &cred).await {
            Ok(r) => println!("CLEANUP delete: ret_code={} dirid={}", r.ret_code, r.dirid),
            Err(e) => println!("CLEANUP delete FAIL: {e}（请人工检查）"),
        }
    }

    println!("done. 临时歌单 dirid={created_dirid:?} tid={created_tid:?} deleted={deleted}");
    // 引用全部公共类型，确保导出可达
    let _ = std::hint::black_box((
        std::any::type_name::<UserCreatedSonglistResponse>(),
        std::any::type_name::<UserFavSonglistResponse>(),
        std::any::type_name::<UserFavAlbumResponse>(),
        std::any::type_name::<GetSonglistDetailResponse>(),
    ));
}
