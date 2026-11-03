//! 歌单链路诊断探针（只读，零写操作）：dump `CgiGetDiss` 原始响应形状，
//! 逐字段对照 `GetSonglistDetailResponse` alias，并核对用户歌单列表键名。
//!
//! 运行：`cargo run -p hmp-qqmusic-api --example live_playlist_diag`
//!
//! 安全：全部为免登录或登录态读接口，绝不调用任何写操作。

use hmp_qqmusic_api::client::QqMusicClient;
use hmp_qqmusic_api::pagination::Page;
use hmp_qqmusic_api::protocol::cgi::CgiRequest;
use hmp_qqmusic_api::songlist::{GetSonglistDetailResponse, SonglistApi};
use hmp_qqmusic_api::user::UserApi;
use serde_json::json;

/// 已知有歌的公共歌单（tests/fixtures/songlist/detail.json 同源）。
const KNOWN_DISSTID: i64 = 7843129912;

fn load_credential() -> Option<hmp_qqmusic_api::credential::Credential> {
    hmp_storage::credential::store_from_env()
        .load()
        .ok()
        .flatten()
        .filter(|c| c.is_logged_in())
}

/// 顶层键与子对象键名摘要。
fn keys_of(v: &serde_json::Value) -> Vec<String> {
    v.as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

#[tokio::main]
async fn main() {
    let client = QqMusicClient::new();
    let songlist = SonglistApi::new(&client);

    // 追加模式：`live_playlist_diag <disstid>...` 只逐歌单打印
    // total（远端口径）供缓存数对照，跳过其余诊断段。
    let tids: Vec<i64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse::<i64>().ok())
        .collect();
    if !tids.is_empty() {
        for tid in tids {
            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
            match songlist
                .get_detail(tid, 0, Page::new(1, 1), true, false, false)
                .await
            {
                Ok(r) => println!("disstid={tid} total_song_num={} code={}", r.total, r.code),
                Err(e) => println!("disstid={tid} FAIL: {e}"),
            }
        }
        return;
    }

    println!("=== 1. CgiGetDiss 原始响应（disstid={KNOWN_DISSTID}, 免登录）===");
    let raw = client
        .musicu_request(
            &CgiRequest::new(
                "music.srfDissInfo.DissInfo",
                "CgiGetDiss",
                json!({
                    "disstid": KNOWN_DISSTID,
                    "dirid": 0,
                    "tag": true,
                    "song_begin": 0,
                    "song_num": 5,
                    "userinfo": true,
                    "orderlist": true,
                    "onlysonglist": false,
                }),
            ),
            None,
        )
        .await;
    match raw {
        Ok(v) => {
            let data = v.get("data").cloned().unwrap_or(json!({}));
            println!("data keys = {:?}", keys_of(&data));
            if let Some(songs) = data.get("songlist").and_then(|s| s.as_array()) {
                println!("data.songlist.len = {}", songs.len());
                if let Some(first) = songs.first() {
                    println!("songlist[0] keys = {:?}", keys_of(first));
                }
            } else {
                println!("!! data.songlist 缺失（或非数组）");
                for alt in ["SongList", "songList", "songs", "list"] {
                    if data.get(alt).is_some() {
                        println!("   备选键 `{alt}` 存在");
                    }
                }
            }
            for key in [
                "total_song_num",
                "totalSongNum",
                "total",
                "songlist_size",
                "songlistSize",
                "hasmore",
                "hasMore",
                "dirinfo",
                "dirInfo",
                "code",
            ] {
                match data.get(key) {
                    Some(val) => println!("data.{key} = {val}"),
                    None => println!("data.{key} 缺失"),
                }
            }
            // 再走解析层对照
            match serde_json::from_value::<GetSonglistDetailResponse>(data) {
                Ok(resp) => println!(
                    "PARSE OK: total={} songs={} size={} hasmore={} title={:?}",
                    resp.total,
                    resp.songs.len(),
                    resp.size,
                    resp.hasmore,
                    resp.info.list.title
                ),
                Err(e) => println!("PARSE FAIL: {e}"),
            }
        }
        Err(e) => println!("WIRE FAIL: {e}"),
    }

    println!("\n=== 2. SonglistApi::get_detail（同参数 API 路径）===");
    match songlist
        .get_detail(KNOWN_DISSTID, 0, Page::new(1, 5), false, true, true)
        .await
    {
        Ok(r) => println!(
            "API OK: code={} total={} songs={} hasmore={} title={:?}",
            r.code,
            r.total,
            r.songs.len(),
            r.hasmore,
            r.info.list.title
        ),
        Err(e) => println!("API FAIL: {e}"),
    }

    let Some(cred) = load_credential() else {
        println!("\ncredential: none（登录态列表核对跳过）");
        return;
    };
    let uin = cred.uin.clone();
    let euin = cred.encrypt_uin.clone();
    println!("\n=== 3. 用户歌单列表键名核对（reconcile 数据源）===");
    let user = UserApi::new(&client);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    match user.get_created_songlist(&uin, Some(&cred)).await {
        Ok(r) => {
            println!(
                "get_created_songlist: total={} parsed={} finished={}",
                r.total,
                r.songlist.len(),
                r.finished
            );
            if let Some(first) = r.songlist.first() {
                println!(
                    "songlist[0]: id={} dirid={} title={:?} songnum={} picurl_empty={}",
                    first.id,
                    first.dirid,
                    first.title,
                    first.songnum,
                    first.picurl.is_empty()
                );
            }
        }
        Err(e) => println!("get_created_songlist FAIL: {e}"),
    }
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    match user
        .get_fav_songlist(&euin, Page::new(1, 10), Some(&cred))
        .await
    {
        Ok(r) => println!(
            "get_fav_songlist: total={} parsed={} hasmore={}",
            r.total,
            r.playlists.len(),
            r.hasmore
        ),
        Err(e) => println!("get_fav_songlist FAIL: {e}"),
    }
}
