//! QQ 用户库快照 reconcile（spec §4）。
//!
//! 拉取「我喜欢 / 自建歌单 / 收藏歌单 / 收藏专辑」→ 合入本地 relations/playlists。
//! 冲突规则：本地存在 pending 意图（outbox 未消费）→ 本地胜、跳过；
//! 否则 QQ snapshot 胜（远端事实写入 desired + last_remote）。
//! 用户看到的永远是本地事实；网络只是后台趋同。

use std::sync::{Arc, Mutex};

use hmp_qqmusic_api::{QqMusicClient, UserApi, credential::Credential};
use hmp_storage::LibraryDb;

/// 拉取 QQ 用户库快照并合入本地（全量分页；任一源失败不阻断其余）。
pub async fn reconcile_user_library(
    client: &QqMusicClient,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
) {
    let euin = credential.encrypt_uin.clone();
    if euin.is_empty() {
        tracing::warn!("凭证缺少 encrypt_uin，跳过 reconcile");
        return;
    }
    let api = UserApi::new(client);
    reconcile_fav_songs(&api, &euin, credential, library).await;
    reconcile_fav_songlists(&api, &euin, credential, library).await;
    reconcile_created_songlists(&api, credential, library).await;
    reconcile_fav_albums(&api, &euin, credential, library).await;
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
        let resp = match api.get_fav_song(euin, page, 100, Some(credential)).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%e, "reconcile: 我喜欢拉取失败");
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
    loop {
        let resp = match api
            .get_fav_songlist(euin, page, 100, Some(credential))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%e, "reconcile: 收藏歌单拉取失败");
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
            tracing::warn!(%e, "reconcile: 自建歌单拉取失败");
            return;
        }
    };
    let Ok(mut lib) = library.lock() else { return };
    for pl in &resp.songlist {
        if pl.id > 0 {
            let remote = if pl.dirid > 0 { pl.dirid } else { pl.id };
            let _ = lib.reconcile_playlist(&remote.to_string(), &pl.title, "owned");
        }
    }
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
        let resp = match api.get_fav_album(euin, page, 100, Some(credential)).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%e, "reconcile: 收藏专辑拉取失败");
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
