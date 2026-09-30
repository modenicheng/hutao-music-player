//! QQ 用户库快照 reconcile（spec §4）。
//!
//! 拉取「我喜欢 / 自建歌单 / 收藏歌单 / 收藏专辑」→ 合入本地 relations/playlists。
//! 冲突规则：本地存在 pending 意图（outbox 未消费）→ 本地胜、跳过；
//! 否则 QQ snapshot 胜（远端事实写入 desired + last_remote）。
//! 用户看到的永远是本地事实；网络只是后台趋同。
//!
//! 附带补齐（每轮限额，幂等）：歌单封面（listing `logo` 优先，缺失走
//! songlist 详情；下载经 `persist_cover` 落本地产物后回写 playlists）与
//! stub 曲目元数据（queue/列表写入的 mid 标题行批量 `query_song` 回填真名）。

use std::sync::{Arc, Mutex};

use hmp_qqmusic_api::pagination::Page;
use hmp_qqmusic_api::song::{SongApi, SongQueryInfo};
use hmp_qqmusic_api::{QqMusicClient, UserApi, credential::Credential};
use hmp_storage::LibraryDb;

/// 每轮 reconcile 最多补抓的歌单封面数（避免冷启动一轮出网过多）。
const PLAYLIST_COVER_BUDGET: usize = 12;
/// 每轮 reconcile 最多修复的 stub 标题行数。
const STUB_REPAIR_BUDGET: usize = 60;

/// 拉取 QQ 用户库快照并合入本地（全量分页；任一源失败不阻断其余）。
pub async fn reconcile_user_library(
    client: &QqMusicClient,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
) {
    let euin = credential.encrypt_uin.clone();
    if euin.is_empty() {
        tracing::warn!("credential missing encrypt_uin; skipping reconcile");
        return;
    }
    let api = UserApi::new(client);
    reconcile_fav_songs(&api, &euin, credential, library).await;
    reconcile_fav_songlists(&api, &euin, credential, library).await;
    reconcile_created_songlists(&api, credential, library).await;
    reconcile_fav_albums(&api, &euin, credential, library).await;
    repair_stub_metadata(client, library).await;
}

/// 下载封面进本地产物目录，返回 `file://` URI（失败 None，下轮重试）。
async fn download_cover(url: &str) -> Option<String> {
    let url = crate::content::normalize_cover_url(url)?;
    let bytes = hmp_media::cdn_client()
        .get(&url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    hmp_storage::scan::persist_cover(&bytes).ok()
}

/// 歌单封面补抓：listing 写入行时就地捕获的 `logo`（SongList.picurl 别名）
/// → 下载落本地产物 → 回写 playlists.cover_uri（仅未设置时写，幂等）。
/// 预算跨 created/subscribed 两组共享；失败（下载/写库）下轮重试。
async fn fill_covers(library: &Arc<Mutex<LibraryDb>>, pairs: &[(String, String)]) {
    let mut budget = PLAYLIST_COVER_BUDGET;
    for (remote_id, url) in pairs {
        if budget == 0 {
            break;
        }
        if url.is_empty() {
            continue;
        }
        let Some(local) = download_cover(url).await else {
            continue;
        };
        let Ok(mut lib) = library.lock() else { return };
        match lib.set_playlist_cover(remote_id, &local) {
            Ok(n) if n > 0 => budget -= 1,
            Ok(_) => {}
            Err(e) => tracing::warn!(%e, remote_id, "reconcile: playlist cover write failed"),
        }
    }
}

/// stub 元数据修复：queue/列表解析写入的行标题是 mid（TrackStub 无名称），
/// 批量 `query_song` 拉真实名称/歌手/专辑回填。upsert 的 COALESCE/CASE
/// 语义保证只补齐不降级；已修复行不再入选，每轮限额直至清零。
async fn repair_stub_metadata(client: &QqMusicClient, library: &Arc<Mutex<LibraryDb>>) {
    let keys: Vec<String> = {
        let Ok(mut lib) = library.lock() else { return };
        match lib.qq_stub_title_keys(STUB_REPAIR_BUDGET as i64) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "reconcile: stub-title query failed");
                return;
            }
        }
    };
    if keys.is_empty() {
        return;
    }
    let queries: Vec<SongQueryInfo> = keys
        .iter()
        .map(|mid| SongQueryInfo {
            id: None,
            mid: Some(mid.clone()),
            song_type: 0,
        })
        .collect();
    let api = SongApi::new(client);
    let songs = match api.query_song(&queries).await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(%e, "reconcile: stub metadata fetch failed");
            return;
        }
    };
    let Ok(mut lib) = library.lock() else { return };
    let mut repaired = 0usize;
    for song in songs {
        if song.name.is_empty() || song.name == song.mid {
            continue;
        }
        let row = hmp_storage::TrackRow {
            source: "qq",
            source_key: song.mid.clone(),
            title: song.name.clone(),
            artist: Some(
                song.singer
                    .iter()
                    .map(|g| g.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            album: (!song.album.name.is_empty()).then(|| song.album.name.clone()),
            cover_uri: None, // COALESCE 保留既有封面；远程 URL 由 CoverGet 回写接管
            ..Default::default()
        };
        match lib.upsert_track(&row) {
            Ok(_) => repaired += 1,
            Err(e) => tracing::warn!(%e, mid = %song.mid, "reconcile: stub repair upsert failed"),
        }
    }
    if repaired > 0 {
        tracing::info!(repaired, "reconcile: stub metadata repaired");
    }
}

/// 「我喜欢」→ relations(track, qq, mid, liked)。逐页取全（hasmore/total）。
async fn reconcile_fav_songs(
    api: &UserApi<'_>,
    euin: &str,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
) {
    let mut page = 1i64;
    let mut present = Vec::new();
    loop {
        let resp = match api
            .get_fav_song(euin, Page::new(page as u32, 100), Some(credential))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%e, "reconcile: failed to fetch liked songs");
                return;
            }
        };
        {
            let Ok(mut lib) = library.lock() else { return };
            for song in resp.songs.iter().filter(|s| !s.mid.is_empty()) {
                present.push(song.mid.clone());
                let _ = lib.reconcile_relation("track", "qq", &song.mid, "liked", true);
                if song.id > 0 {
                    let _ = lib.set_track_qq_song_id("qq", &song.mid, song.id);
                }
            }
        }
        if resp.hasmore == 0 || page >= 100 || resp.songs.is_empty() {
            break;
        }
        page += 1;
    }
    // 双向 reconcile：远端已取消收藏的本地行（synced）→ desired=0。
    if let Ok(mut lib) = library.lock() {
        let _ = lib.reconcile_remove_absent("track", "qq", "liked", &present);
    }
}

/// 收藏歌单 → relations(playlist, subscribed) + playlists 行（remote_id=disstid）。
async fn reconcile_fav_songlists(
    api: &UserApi<'_>,
    euin: &str,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
) {
    let mut page = 1i64;
    let mut present = Vec::new();
    let mut covers: Vec<(String, String)> = Vec::new();
    loop {
        let resp = match api
            .get_fav_songlist(euin, Page::new(page as u32, 100), Some(credential))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%e, "reconcile: failed to fetch subscribed playlists");
                return;
            }
        };
        {
            let Ok(mut lib) = library.lock() else { return };
            for pl in &resp.playlists {
                if pl.id > 0 {
                    present.push(pl.id.to_string());
                    let _ = lib.reconcile_relation(
                        "playlist",
                        "qq",
                        &pl.id.to_string(),
                        "subscribed",
                        true,
                    );
                    let _ = lib.reconcile_playlist(&pl.id.to_string(), &pl.title, "subscribed");
                    // listing 就地捕获封面（subscribed 的 remote_id 即 disstid）
                    covers.push((pl.id.to_string(), pl.picurl.clone()));
                }
            }
        }
        if resp.hasmore == 0 || page >= 100 || resp.playlists.is_empty() {
            break;
        }
        page += 1;
    }
    // 双向：远端已取消收藏的歌单（synced 行）→ desired=0。
    if let Ok(mut lib) = library.lock() {
        let _ = lib.reconcile_remove_absent("playlist", "qq", "subscribed", &present);
        // 远端缺席的歌单同步删除本地行（不留幽灵 subscribed 条目）。
        let _ = lib.delete_playlists_absent("subscribed", &present);
    }
    fill_covers(library, &covers).await;
}

/// 自建歌单 → playlists 行（relation=owned，remote_id=dirid；dirid 缺失回退 id）。
async fn reconcile_created_songlists(
    api: &UserApi<'_>,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
) {
    let resp = match api
        .get_created_songlist(&credential.uin, Some(credential))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "reconcile: failed to fetch created playlists");
            return;
        }
    };
    let covers: Vec<(String, String)> = {
        let Ok(mut lib) = library.lock() else { return };
        let mut covers: Vec<(String, String)> = Vec::new();
        for pl in &resp.songlist {
            if pl.id > 0 {
                let remote = if pl.dirid > 0 { pl.dirid } else { pl.id };
                let _ = lib.reconcile_playlist(&remote.to_string(), &pl.title, "owned");
                // listing 就地捕获封面（行键与 remote_id 同一形态：dirid 优先）
                covers.push((remote.to_string(), pl.picurl.clone()));
            }
        }
        covers
    };
    fill_covers(library, &covers).await;
}

/// 收藏专辑 → relations(album, qq, album_id, liked)。
async fn reconcile_fav_albums(
    api: &UserApi<'_>,
    euin: &str,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
) {
    let mut page = 1i64;
    let mut present = Vec::new();
    loop {
        let resp = match api
            .get_fav_album(euin, Page::new(page as u32, 100), Some(credential))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%e, "reconcile: failed to fetch liked albums");
                return;
            }
        };
        {
            let Ok(mut lib) = library.lock() else { return };
            for album in &resp.albums {
                if album.id > 0 {
                    present.push(album.id.to_string());
                    let _ =
                        lib.reconcile_relation("album", "qq", &album.id.to_string(), "liked", true);
                }
            }
        }
        if resp.hasmore == 0 || page >= 100 || resp.albums.is_empty() {
            break;
        }
        page += 1;
    }
    // 双向：远端已取消收藏的专辑（synced 行）→ desired=0。
    if let Ok(mut lib) = library.lock() {
        let _ = lib.reconcile_remove_absent("album", "qq", "liked", &present);
    }
}

#[cfg(test)]
mod tests {

    /// 宽松反序列化：缺失字段走 default，alias 生效。
    #[test]
    fn fav_album_response_parses_loosely() {
        let v: hmp_qqmusic_api::UserFavAlbumResponse = serde_json::from_value(serde_json::json!({
            "vecAlbum": [{"albumID": 123, "albumName": "叶惠美"}],
            "hasmore": 1,
            "total": 5,
        }))
        .expect("宽松反序列化");
        assert_eq!(v.albums.len(), 1);
        assert_eq!(v.albums[0].id, 123);
    }

    /// reconcile 的 pending 优先规则（storage 层单测）。
    #[test]
    fn reconcile_keeps_pending_local_intent() {
        use hmp_storage::LibraryDb;
        let mut db = LibraryDb::open_in_memory().unwrap();
        // 本地意图（pending）
        db.add_favorite("qq", "mid-a", "mid-a").unwrap();
        // 远端快照说：该曲已收藏 —— 不应覆盖 pending 意图的 desired。
        db.reconcile_relation("track", "qq", "mid-a", "liked", true)
            .unwrap();
        let row = db
            .relations_pending()
            .unwrap()
            .into_iter()
            .find(|r| r.entity_key == "mid-a")
            .expect("本地意图应保留 pending");
        assert!(row.desired_state);
        assert_eq!(row.sync_state, "pending");
        // 无本地意图的曲目：远端胜。
        db.reconcile_relation("track", "qq", "mid-remote", "liked", true)
            .unwrap();
        assert_eq!(
            db.relation_desired("track", "qq", "mid-remote", "liked")
                .unwrap(),
            Some(true)
        );
        // mid-remote 已 synced，不进 outbox
        let pending: Vec<_> = db
            .relations_pending()
            .unwrap()
            .into_iter()
            .filter(|r| r.entity_key == "mid-remote")
            .collect();
        assert!(pending.is_empty());
    }
}
