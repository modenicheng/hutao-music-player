//! QQ 用户库快照 reconcile（spec §4）。
//!
//! 拉取「我喜欢 / 自建歌单 / 收藏歌单 / 收藏专辑」→ 合入本地 relations/playlists。
//! 冲突规则：本地存在 pending 意图（outbox 未消费）→ 本地胜、跳过；
//! 否则 QQ snapshot 胜（远端事实写入 desired + last_remote）。
//! 用户看到的永远是本地事实；网络只是后台趋同。
//!
//! 附带补齐（每轮限额，幂等）：歌单封面（listing `logo` 优先，缺失走
//! songlist 详情；下载经 `persist_cover` 落本地产物后回写 playlists）、
//! stub 曲目元数据（queue/列表写入的 mid 标题行批量 `query_song` 回填真名）
//! 与**歌单曲目缓存**（listing 只建 playlists 行不建曲目行；本地库歌单页/
//! 详情页/`playlist:local:<id>` 播放全读 sqlite，无曲目缓存则全部曲目数为
//! 0——2026-10-02 修复「所有歌单无法获取歌曲」的缺腿）。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use hmp_qqmusic_api::models::Song;
use hmp_qqmusic_api::pagination::Page;
use hmp_qqmusic_api::song::{SongApi, SongQueryInfo};
use hmp_qqmusic_api::songlist::SonglistApi;
use hmp_qqmusic_api::{QqMusicClient, UserApi, credential::Credential};
use hmp_storage::LibraryDb;

/// 每轮 reconcile 最多补抓的歌单封面数（避免冷启动一轮出网过多）。
const PLAYLIST_COVER_BUDGET: usize = 12;
/// 每轮 reconcile 最多修复的 stub 标题行数。
const STUB_REPAIR_BUDGET: usize = 60;
/// 每轮 reconcile 最多补抓曲目的歌单数（每歌单 1 页/百首起，冷启动预算）。
const PLAYLIST_TRACKS_BUDGET: usize = 24;
/// 单歌单曲目补抓的最大页数（100 首/页 → 2000 首封顶；超限视为本次抓取
/// 不完整，**放弃落库**——半窗快照做差集会把未取回的曲目误判远端已删）。
const PLAYLIST_TRACKS_MAX_PAGES: u32 = 20;
/// 「我喜欢」歌单的 remote_id（目录 ID 固定 201；`CgiGetDiss` 对它须走
/// `dirid=201 + enc_host_uin` 形态而非 disstid，见 [`UserApi::get_fav_song`]）。
const FAV_SONG_DIRID: &str = "201";

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
    let owned_tids = reconcile_created_songlists(&api, credential, library).await;
    reconcile_fav_albums(&api, &euin, credential, library).await;
    repair_stub_metadata(client, library).await;
    // 歌单曲目缓存（listing 之后再跑：playlists 行须先就位）。
    reconcile_playlist_tracks(client, credential, library, &owned_tids).await;
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
        // 先清子行再删歌单（AUDIT §4.4 FK 地雷：曲目缓存落地后
        // delete_playlists_absent 的父行删除会因 playlist_tracks 外键失败）。
        purge_tracks_of_absent_playlists(&mut lib, "subscribed", &present);
        // 远端缺席的歌单同步删除本地行（不留幽灵 subscribed 条目）。
        let _ = lib.delete_playlists_absent("subscribed", &present);
    }
    fill_covers(library, &covers).await;
}

/// 清空即将被 [`LibraryDb::delete_playlists_absent`] 删除的歌单的曲目行
/// （同一谓词：relation + synced + remote_id 缺席；降序移除保证 position 有效）。
fn purge_tracks_of_absent_playlists(lib: &mut LibraryDb, relation: &str, present_keys: &[String]) {
    let absent: Vec<i64> = lib
        .list_playlists()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| {
            p.relation == relation
                && p.sync_state == "synced"
                && p.remote_id
                    .as_deref()
                    .is_some_and(|rid| !present_keys.iter().any(|k| k == rid))
        })
        .map(|p| p.id)
        .collect();
    for row_id in absent {
        let rows = match lib.playlist_tracks(row_id) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(%e, row_id, "reconcile: absent playlist tracks read failed");
                continue;
            }
        };
        for row in rows.into_iter().rev() {
            let _ = lib.remove_playlist_track(row_id, row.position);
        }
    }
}

/// 自建歌单 → playlists 行（relation=owned，remote_id=dirid；dirid 缺失回退 id）。
/// 返回 `(remote_id, tid)` 映射（曲目补抓用：playlists.remote_id 存 dirid 是
/// 写操作语义（add/del/delete 都吃 dirid），而 `CgiGetDiss` 只认 disstid/tid；
/// listing 同时携带两者，就地换算免二次出网）。
async fn reconcile_created_songlists(
    api: &UserApi<'_>,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
) -> Vec<(String, i64)> {
    let resp = match api
        .get_created_songlist(&credential.uin, Some(credential))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "reconcile: failed to fetch created playlists");
            return Vec::new();
        }
    };
    let mut tids: Vec<(String, i64)> = Vec::new();
    let covers: Vec<(String, String)> = {
        let Ok(mut lib) = library.lock() else {
            return Vec::new();
        };
        let mut covers: Vec<(String, String)> = Vec::new();
        for pl in &resp.songlist {
            if pl.id > 0 {
                let remote = if pl.dirid > 0 { pl.dirid } else { pl.id };
                let _ = lib.reconcile_playlist(&remote.to_string(), &pl.title, "owned");
                // listing 就地捕获封面（行键与 remote_id 同一形态：dirid 优先）
                covers.push((remote.to_string(), pl.picurl.clone()));
                tids.push((remote.to_string(), pl.id));
            }
        }
        covers
    };
    fill_covers(library, &covers).await;
    tids
}

/// 歌单曲目缓存：逐歌单拉远端曲目快照（`CgiGetDiss` 分页），差集合入
/// `playlist_tracks`（幂等：远端新增追加、远端已删移除、元数据刷新走
/// upsert 的 COALESCE 只补齐不降级；本地 `local:` 行永不触碰）。
///
/// 冲突规则与 listing reconcile 一致：歌单存在未消费的本地意图
/// （owned 建单/曲目增删 outbox 行、subscribed 收藏 relation 行）→ 本地胜、
/// 本轮跳过；否则远端快照胜。每轮预算 [`PLAYLIST_TRACKS_BUDGET`] 个歌单，
/// 下一轮 reconcile 续跑；抓取失败保留本地现状（绝不因网络问题清库）。
async fn reconcile_playlist_tracks(
    client: &QqMusicClient,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
    owned_tids: &[(String, i64)],
) {
    // 行快照 + 本地意图集合（先锁读、后出网，不持锁跨 await）。
    let rows: Vec<(i64, String, String)> = {
        let Ok(mut lib) = library.lock() else {
            return;
        };
        lib.list_playlists()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| {
                let remote_id = p.remote_id?;
                (p.relation == "owned" || p.relation == "subscribed")
                    .then_some((p.id, remote_id, p.relation))
            })
            .collect()
    };
    let pending_ops: HashSet<i64> = {
        let Ok(mut lib) = library.lock() else {
            return;
        };
        lib.playlist_ops_pending()
            .unwrap_or_default()
            .into_iter()
            .map(|op| op.playlist_id)
            .collect()
    };
    let pending_playlists: HashSet<String> = {
        let Ok(mut lib) = library.lock() else {
            return;
        };
        lib.relations_pending()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.entity_type == "playlist" && r.relation == "subscribed")
            .map(|r| r.entity_key)
            .collect()
    };

    let mut budget = PLAYLIST_TRACKS_BUDGET;
    for (row_id, remote_id, relation) in rows {
        if budget == 0 {
            tracing::info!("reconcile: playlist track budget exhausted; next round continues");
            break;
        }
        if pending_ops.contains(&row_id) || pending_playlists.contains(&remote_id) {
            continue; // 本地意图未消费：本地胜出，不动曲目
        }
        let tid = match relation.as_str() {
            "owned" if remote_id == FAV_SONG_DIRID => None, // 我喜欢走 get_fav_song
            "owned" => owned_tids
                .iter()
                .find(|(key, _)| key == &remote_id)
                .map(|(_, tid)| *tid),
            _ => remote_id.parse::<i64>().ok(), // subscribed：remote_id 即 disstid
        };
        let songs = match fetch_playlist_songs(
            client,
            credential,
            &credential.encrypt_uin,
            &remote_id,
            tid,
        )
        .await
        {
            Some(songs) => songs,
            None => continue, // 抓取失败/不完整：保留本地现状，下轮重试
        };
        budget -= 1;
        let (added, removed) = apply_playlist_track_snapshot(library, row_id, &songs);
        if added + removed > 0 {
            tracing::debug!(row_id, %remote_id, %relation, added, removed,
                "reconcile: playlist tracks synced");
        }
    }
}

/// 拉一个歌单的远端曲目快照（有序；超页上限/中途失败 → None）。
///
/// 「我喜欢」（remote_id=201）是目录而非普通 disstid：`CgiGetDiss` 须以
/// `dirid=201 + enc_host_uin` 形态请求（[`UserApi::get_fav_song`] 同款）；
/// 其余歌单 owned 行以 listing 换算的 tid、subscribed 行以 remote_id
/// （即 disstid）走 [`SonglistApi::get_detail`]（onlysong 对齐 player.rs 播放路径）。
async fn fetch_playlist_songs(
    client: &QqMusicClient,
    credential: &Credential,
    euin: &str,
    remote_id: &str,
    tid: Option<i64>,
) -> Option<Vec<Song>> {
    let is_fav = remote_id == FAV_SONG_DIRID;
    if !is_fav && tid.is_none() {
        tracing::warn!(remote_id, "reconcile: playlist tid unknown; tracks skipped");
        return None;
    }
    let user = UserApi::new(client);
    let songlist = SonglistApi::new(client);
    let mut songs = Vec::new();
    for page in 1..=PLAYLIST_TRACKS_MAX_PAGES {
        let resp = if is_fav {
            user.get_fav_song(euin, Page::new(page, 100), Some(credential))
                .await
        } else {
            songlist
                .get_detail(
                    tid.unwrap_or_default(),
                    0,
                    Page::new(page, 100),
                    true,
                    false,
                    false,
                )
                .await
        };
        match resp {
            Ok(r) => {
                let page_len = r.songs.len();
                songs.extend(r.songs);
                if r.hasmore == 0 || page_len == 0 {
                    return Some(songs);
                }
            }
            Err(e) => {
                tracing::warn!(remote_id, page, %e, "reconcile: playlist tracks fetch failed");
                return None;
            }
        }
    }
    tracing::warn!(
        remote_id,
        PLAYLIST_TRACKS_MAX_PAGES,
        "reconcile: playlist exceeds page cap; tracks skipped this round"
    );
    None
}

/// 远端快照差集合入本地歌单曲目（锁内无 await；返回 (added, removed)）。
fn apply_playlist_track_snapshot(
    library: &Arc<Mutex<LibraryDb>>,
    row_id: i64,
    songs: &[Song],
) -> (usize, usize) {
    let remote: Vec<&Song> = songs.iter().filter(|s| s.has_playable_identity()).collect();
    let remote_keys: HashSet<&str> = remote.iter().map(|s| s.mid.as_str()).collect();
    let Ok(mut lib) = library.lock() else {
        return (0, 0);
    };
    let local_rows = match lib.playlist_tracks(row_id) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(%e, row_id, "reconcile: playlist tracks read failed");
            return (0, 0);
        }
    };
    // 远端已删：本地 QQ 行（local: 行属本地事实，永不触碰）缺席远端 → 降序移除
    // （降序保证未处理行 position 不因删除位移失效）。
    let mut absent: Vec<i64> = local_rows
        .iter()
        .filter(|row| {
            !row.source_key.starts_with("local:") && !remote_keys.contains(row.source_key.as_str())
        })
        .map(|row| row.position)
        .collect();
    absent.sort_unstable_by(|a, b| b.cmp(a));
    let mut removed = 0usize;
    for position in absent {
        if lib.remove_playlist_track(row_id, position).is_ok() {
            removed += 1;
        }
    }
    // 远端新增/既有：元数据全量 upsert（COALESCE 只补齐不降级）+ 缺失链接追加
    //（add_playlist_track 幂等去重，同曲不重复入列）。
    let local_keys: HashSet<String> = local_rows
        .iter()
        .map(|row| row.source_key.clone())
        .collect();
    let mut added = 0usize;
    for song in &remote {
        if lib.upsert_track(&qq_track_row(song)).is_err() {
            continue;
        }
        if !local_keys.contains(&song.mid)
            && lib
                .add_playlist_track(row_id, "qq", &song.mid, &song.name)
                .is_ok()
        {
            added += 1;
        }
    }
    (added, removed)
}

/// 远端 `Song` → 全量元数据曲目行（入库后桌面/CLI 经 track_meta_batch 补全展示）。
fn qq_track_row(song: &Song) -> hmp_storage::TrackRow {
    hmp_storage::TrackRow {
        source: "qq",
        source_key: song.mid.clone(),
        title: song.name.clone(),
        artist: (!song.singer.is_empty()).then(|| {
            song.singer
                .iter()
                .map(|g| g.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }),
        album: (!song.album.name.is_empty()).then(|| song.album.name.clone()),
        duration_ms: (song.interval > 0).then(|| song.interval * 1000),
        cover_uri: None, // 远程 URL 由 CoverGet 域守卫链路接管，不入库
        qq_song_id: (song.id > 0).then_some(song.id),
        ..Default::default()
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
    use super::*;

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

    /// 构造远端快照歌曲（mid/name/歌手/专辑/时长齐全，可播放身份）。
    fn snapshot_song(mid: &str, name: &str, artist: &str, album: &str, interval: i64) -> Song {
        Song {
            id: 42,
            mid: mid.to_owned(),
            name: name.to_owned(),
            interval,
            singer: vec![hmp_qqmusic_api::models::Singer {
                name: artist.to_owned(),
                ..Default::default()
            }],
            album: hmp_qqmusic_api::models::Album {
                name: album.to_owned(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// 歌单曲目缓存：空歌单 → 快照全量入列（远端顺序 + 全量元数据）。
    #[test]
    fn playlist_track_snapshot_fills_empty_playlist() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let row_id = lib
            .lock()
            .unwrap()
            .reconcile_playlist("7", "测试歌单", "owned")
            .unwrap();
        let songs = vec![
            snapshot_song("m-1", "歌一", "歌手A", "专辑A", 180),
            snapshot_song("m-2", "歌二", "歌手B", "专辑B", 200),
        ];
        let (added, removed) = apply_playlist_track_snapshot(&lib, row_id, &songs);
        assert_eq!((added, removed), (2, 0));
        let mut db = lib.lock().unwrap();
        let rows = db.playlist_tracks(row_id).unwrap();
        let keys: Vec<_> = rows.iter().map(|r| r.source_key.as_str()).collect();
        assert_eq!(keys, ["m-1", "m-2"], "远端顺序入列");
        assert_eq!(db.list_playlists().unwrap()[0].track_count, 2);
        // 元数据入库（桌面/CLI 经 track_meta_batch 补全展示）。
        let meta = db.track_meta_batch("qq", &["m-1".to_owned()]).unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].title, "歌一");
        assert_eq!(meta[0].artist.as_deref(), Some("歌手A"));
        assert_eq!(meta[0].album.as_deref(), Some("专辑A"));
        assert_eq!(meta[0].duration_ms, Some(180_000));
    }

    /// 幂等：同快照重放零变更、不重复入列。
    #[test]
    fn playlist_track_snapshot_is_idempotent() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let row_id = lib
            .lock()
            .unwrap()
            .reconcile_playlist("7", "测试歌单", "owned")
            .unwrap();
        let songs = vec![
            snapshot_song("m-1", "歌一", "歌手A", "专辑A", 180),
            snapshot_song("m-2", "歌二", "歌手B", "专辑B", 200),
        ];
        assert_eq!(apply_playlist_track_snapshot(&lib, row_id, &songs), (2, 0));
        assert_eq!(apply_playlist_track_snapshot(&lib, row_id, &songs), (0, 0));
        assert_eq!(
            lib.lock().unwrap().playlist_tracks(row_id).unwrap().len(),
            2
        );
    }

    /// 差集：远端新增追加、远端已删移除；本地 `local:` 行属本地事实，永不触碰。
    #[test]
    fn playlist_track_snapshot_diffs_and_keeps_local() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let row_id = lib
            .lock()
            .unwrap()
            .reconcile_playlist("7", "混排歌单", "owned")
            .unwrap();
        let one = vec![snapshot_song("m-1", "歌一", "歌手A", "专辑A", 180)];
        let two = vec![
            snapshot_song("m-1", "歌一", "歌手A", "专辑A", 180),
            snapshot_song("m-2", "歌二", "歌手B", "专辑B", 200),
        ];
        assert_eq!(apply_playlist_track_snapshot(&lib, row_id, &one), (1, 0));
        lib.lock()
            .unwrap()
            .add_playlist_track(row_id, "local", "local:/x.mp3", "本地歌")
            .unwrap();
        assert_eq!(apply_playlist_track_snapshot(&lib, row_id, &two), (1, 0));
        // 收窄快照：m-2 远端已删 → 移除；local 行保留。
        assert_eq!(apply_playlist_track_snapshot(&lib, row_id, &one), (0, 1));
        let mut db = lib.lock().unwrap();
        let keys: Vec<_> = db
            .playlist_tracks(row_id)
            .unwrap()
            .into_iter()
            .map(|r| r.source_key)
            .collect();
        assert_eq!(keys, ["m-1", "local:/x.mp3"], "QQ 行移除、本地行保留");
    }

    /// 无播放身份的快照条目（缺 mid/name）不入列。
    #[test]
    fn playlist_track_snapshot_skips_unplayable_entries() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let row_id = lib
            .lock()
            .unwrap()
            .reconcile_playlist("7", "测试歌单", "owned")
            .unwrap();
        let songs = vec![Song {
            mid: "m-no-name".to_owned(),
            ..Default::default()
        }];
        assert_eq!(apply_playlist_track_snapshot(&lib, row_id, &songs), (0, 0));
        assert!(
            lib.lock()
                .unwrap()
                .playlist_tracks(row_id)
                .unwrap()
                .is_empty()
        );
    }

    /// FK 地雷拆除：曲目缓存落地后，远端缺席 subscribed 歌单须先清子行，
    /// delete_playlists_absent 才能成功删父行（AUDIT §4.4）。
    #[test]
    fn absent_subscribed_playlist_tracks_purged_before_delete() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let row_id = lib
            .lock()
            .unwrap()
            .reconcile_playlist("8888", "取消收藏的歌单", "subscribed")
            .unwrap();
        let songs = vec![snapshot_song("m-8", "歌八", "歌手H", "专辑H", 90)];
        assert_eq!(apply_playlist_track_snapshot(&lib, row_id, &songs), (1, 0));
        // 远端仍收藏（present 含 8888）：子行保留。
        {
            let mut db = lib.lock().unwrap();
            purge_tracks_of_absent_playlists(&mut db, "subscribed", &["8888".to_owned()]);
            assert_eq!(db.playlist_tracks(row_id).unwrap().len(), 1);
        }
        // 远端已取消收藏：先清子行 → 父行删除成功（无 FK 错误）。
        {
            let mut db = lib.lock().unwrap();
            purge_tracks_of_absent_playlists(&mut db, "subscribed", &["9999".to_owned()]);
            db.delete_playlists_absent("subscribed", &["9999".to_owned()])
                .expect("子行清理后父行删除不应 FK 失败");
            assert!(db.list_playlists().unwrap().is_empty());
        }
    }
}
