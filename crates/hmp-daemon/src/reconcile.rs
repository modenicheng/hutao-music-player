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
/// 空快照须连续一致的轮数：达到才允许按空快照清理本地（AUDIT §30 D2）。
const EMPTY_SNAPSHOT_ROUNDS: u32 = 2;

/// 空快照闸门（AUDIT §30 D2）：存储层对空集合的语义是「全量清理 synced 行」，
/// 而接口软失败会被解析成空列表——故空快照须**连续** [`EMPTY_SNAPSHOT_ROUNDS`]
/// 轮一致才视为权威空。非空快照清零并放行；首个空快照只计数、不放行。
/// 拉取失败 / 不完整的轮次不经过本闸门（无信息量），计数保持。
#[derive(Debug, Default)]
struct EmptySnapshotGate {
    /// 连续为空的轮数。
    empty_rounds: u32,
}

impl EmptySnapshotGate {
    /// 记录本轮快照条数，返回本轮是否允许按该快照清理本地。
    fn observe(&mut self, present_len: usize) -> bool {
        if present_len > 0 {
            self.empty_rounds = 0;
            return true;
        }
        self.empty_rounds = self.empty_rounds.saturating_add(1);
        self.empty_rounds >= EMPTY_SNAPSHOT_ROUNDS
    }
}

/// 闸门判定 + 跳过清理时的告警（三个快照源共用）。
///
/// `complete`：快照是否取全（`hasmore=0` 收尾）。半份快照同样不能定缺席——
/// 翻页中途的空页会让 `present` 只剩前几页，按它清理等于把没拉到的条目全当
/// 缺席清掉（与 §30.1 的分片清理同一类症状）；只做增量，清理留到取全的那轮。
fn snapshot_allows_cleanup(
    gate: &mut EmptySnapshotGate,
    present_len: usize,
    complete: bool,
    source: &str,
) -> bool {
    if !complete {
        tracing::warn!(
            source,
            present = present_len,
            "reconcile: incomplete snapshot; absent cleanup skipped"
        );
        return false;
    }
    if gate.observe(present_len) {
        return true;
    }
    tracing::warn!(
        source,
        empty_rounds = gate.empty_rounds,
        "reconcile: empty snapshot; cleanup deferred until it repeats"
    );
    false
}

/// SyncWorker 持有的 reconcile 跨轮状态：空快照闸门与歌单曲目补抓游标。
/// 仅内存、不入库——daemon 重启后计数与游标清零（补抓从头开始，可接受）。
#[derive(Debug, Default)]
pub struct ReconcileState {
    /// 「我喜欢」/ 收藏歌单 / 收藏专辑各自的空快照闸门。
    fav_songs: EmptySnapshotGate,
    fav_songlists: EmptySnapshotGate,
    fav_albums: EmptySnapshotGate,
    /// 歌单曲目补抓游标（远端歌单行下标；本遍走完归零，见 [`plan_slice`]）。
    tracks_cursor: usize,
    /// 本遍的 owned 歌单 `(remote_id, tid)` 映射：续跑片复用，免重复拉 listing。
    owned_tids: Vec<(String, i64)>,
}

/// 拉取 QQ 用户库快照并合入本地（全量分页；任一源失败不阻断其余）。
/// `state` 为跨轮内存状态（闸门计数、补抓游标），由调用方（SyncWorker）持有。
/// 返回歌单曲目补抓本遍是否还有未处理的行（true → 调用方短延时后续跑，
/// 见 [`resume_playlist_tracks`]）。
pub async fn reconcile_user_library(
    client: &QqMusicClient,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
    state: &mut ReconcileState,
) -> bool {
    let euin = credential.encrypt_uin.clone();
    if euin.is_empty() {
        tracing::warn!("credential missing encrypt_uin; skipping reconcile");
        return false;
    }
    let api = UserApi::new(client);
    reconcile_fav_songs(&api, &euin, credential, library, &mut state.fav_songs).await;
    reconcile_fav_songlists(&api, &euin, credential, library, &mut state.fav_songlists).await;
    let owned_tids = reconcile_created_songlists(&api, credential, library).await;
    state.owned_tids = owned_tids;
    reconcile_fav_albums(&api, &euin, credential, library, &mut state.fav_albums).await;
    repair_stub_metadata(client, library).await;
    // 歌单曲目缓存（listing 之后再跑：playlists 行须先就位）。
    reconcile_playlist_tracks(
        client,
        credential,
        library,
        &state.owned_tids,
        &mut state.tracks_cursor,
    )
    .await
    .1
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
        // 与 reconcile 主入库同一投影：mid 一起补齐（COALESCE 不降级）。
        let row = qq_track_row(&song);
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
/// 空快照经 `gate` 判定（连续两轮为空才清理）；完整快照后收口已在场的
/// liked 意图行（[`close_present_liked_intents`]）。
async fn reconcile_fav_songs(
    api: &UserApi<'_>,
    euin: &str,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
    gate: &mut EmptySnapshotGate,
) {
    let mut page = 1i64;
    let mut present = Vec::new();
    // 快照是否取全：`hasmore=0` 收尾才算完整；页数封顶 / 空页提前止不算。
    let mut complete = false;
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
        if resp.hasmore == 0 {
            complete = true;
            break;
        }
        if page >= 100 || resp.songs.is_empty() {
            break;
        }
        page += 1;
    }
    // 双向 reconcile：远端已取消收藏的本地行（synced）→ desired=0。
    // 只有取全的快照才定缺席；空快照须连续两轮一致才清理（软失败被解析成
    // 空列表时保留本地）。
    if snapshot_allows_cleanup(gate, present.len(), complete, "liked songs")
        && let Ok(mut lib) = library.lock()
    {
        let _ = lib.reconcile_remove_absent("track", "qq", "liked", &present);
    }
    if complete {
        close_present_liked_intents(library, &present);
    }
}

/// 关闭卡死的 liked 意图行：远端「我喜欢」**在场**的曲目，若本地关系行仍非
/// synced 且意图为收藏（`desired=1`，如 QQ 返回 1101 一直退避重试的 error 行），
/// 说明远端已反映该意图 → 标 synced。`reconcile_relation` 对非 synced 行是
/// 本地意图优先，不经此收口永远不会合拢。只处理「在场 + desired=1」；缺席
/// 方向需要完整快照前提，不在本函数处理。返回收口行数。
fn close_present_liked_intents(library: &Arc<Mutex<LibraryDb>>, present: &[String]) -> usize {
    let present: HashSet<&str> = present.iter().map(String::as_str).collect();
    let Ok(mut lib) = library.lock() else {
        return 0;
    };
    let stuck: Vec<String> = match lib.relations_pending() {
        Ok(rows) => rows
            .into_iter()
            .filter(|r| {
                r.entity_type == "track"
                    && r.provider == "qq"
                    && r.relation == "liked"
                    && r.desired_state
                    && present.contains(r.entity_key.as_str())
            })
            .map(|r| r.entity_key)
            .collect(),
        Err(e) => {
            tracing::warn!(%e, "reconcile: stuck liked intents query failed");
            return 0;
        }
    };
    let mut closed = 0usize;
    for key in &stuck {
        match lib.mark_relation_synced("track", "qq", key, "liked") {
            Ok(()) => closed += 1,
            Err(e) => tracing::warn!(%e, key, "reconcile: stuck liked intent close failed"),
        }
    }
    closed
}

/// 收藏歌单 → relations(playlist, subscribed) + playlists 行（remote_id=disstid）。
/// 空快照经 `gate` 判定（连续两轮为空才清理）。
async fn reconcile_fav_songlists(
    api: &UserApi<'_>,
    euin: &str,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
    gate: &mut EmptySnapshotGate,
) {
    let mut page = 1i64;
    let mut present = Vec::new();
    let mut covers: Vec<(String, String)> = Vec::new();
    // 快照是否取全：`hasmore=0` 收尾才算完整（同 `reconcile_fav_songs`）。
    let mut complete = false;
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
        if resp.hasmore == 0 {
            complete = true;
            break;
        }
        if page >= 100 || resp.playlists.is_empty() {
            break;
        }
        page += 1;
    }
    // 双向：远端已取消收藏的歌单（synced 行）→ desired=0。
    // 只有取全的快照才定缺席（半份快照会连带删掉本地歌单行与曲目缓存）；
    // 空快照须连续两轮一致才清理（软失败被解析成空列表时保留本地）。
    if snapshot_allows_cleanup(gate, present.len(), complete, "subscribed playlists")
        && let Ok(mut lib) = library.lock()
    {
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
/// 本轮跳过；否则远端快照胜。每轮处理自 `cursor` 起一片（切片见
/// [`plan_slice`]；未走完时 SyncWorker 短延时续跑，见 [`resume_playlist_tracks`]）——
/// 此前每轮都从第 0 行起，排在预算之后的歌单永远补不到曲目（AUDIT §30 D5）。
/// 抓取失败保留本地现状（绝不因网络问题清库），本遍内不重试、游标回卷后的
/// 下一遍再试。返回 `(变更行数, 本遍是否还有未处理的行)`。
async fn reconcile_playlist_tracks(
    client: &QqMusicClient,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
    owned_tids: &[(String, i64)],
    cursor: &mut usize,
) -> (usize, bool) {
    // 行快照 + 本地意图集合（先锁读、后出网，不持锁跨 await）。
    let rows: Vec<(i64, String, String)> = {
        let Ok(mut lib) = library.lock() else {
            return (0, false);
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
    let (start, end, next_cursor, has_more) =
        plan_slice(rows.len(), *cursor, PLAYLIST_TRACKS_BUDGET);
    let pending_ops: HashSet<i64> = {
        let Ok(mut lib) = library.lock() else {
            return (0, false);
        };
        lib.playlist_ops_pending()
            .unwrap_or_default()
            .into_iter()
            .map(|op| op.playlist_id)
            .collect()
    };
    let pending_playlists: HashSet<String> = {
        let Ok(mut lib) = library.lock() else {
            return (0, false);
        };
        lib.relations_pending()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.entity_type == "playlist" && r.relation == "subscribed")
            .map(|r| r.entity_key)
            .collect()
    };

    let mut changed = 0usize;
    for (row_id, remote_id, relation) in rows[start..end].iter().cloned() {
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
            None => continue, // 抓取失败/不完整：保留本地现状，下一遍重试
        };
        let (added, removed) = apply_playlist_track_snapshot(library, row_id, &songs);
        changed += added + removed;
        if added + removed > 0 {
            tracing::debug!(row_id, %remote_id, %relation, added, removed,
                "reconcile: playlist tracks synced");
        }
    }
    if has_more {
        tracing::info!(
            start,
            end,
            total = rows.len(),
            "reconcile: playlist track budget spent; resuming from the cursor shortly"
        );
    }
    *cursor = next_cursor;
    (changed, has_more)
}

/// 歌单曲目补抓的续跑片：预算用完、本遍未走完时由 SyncWorker 短延时后调用
/// （只跑曲目补抓，复用本遍已拉的 owned 映射；其余快照源不重复出网）。
/// 返回值同 `reconcile_playlist_tracks`：`(变更行数, 本遍是否还有未处理的行)`。
pub async fn resume_playlist_tracks(
    client: &QqMusicClient,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
    state: &mut ReconcileState,
) -> (usize, bool) {
    reconcile_playlist_tracks(
        client,
        credential,
        library,
        &state.owned_tids,
        &mut state.tracks_cursor,
    )
    .await
}

/// 歌单曲目补抓的切片计划（纯函数）：在 `rows_len` 行里自游标 `cursor` 起取至多
/// `budget` 行，返回 `(start, end, next_cursor, has_more)`。
/// - 游标越过行尾（本遍中途行数减少）→ 本轮直接从第 0 行起；
/// - `has_more`：本遍还有未处理的行，`next_cursor = end`（下轮续跑）；
///   否则本遍走完，`next_cursor` 归零（下个触发从头刷新）。
fn plan_slice(rows_len: usize, cursor: usize, budget: usize) -> (usize, usize, usize, bool) {
    let start = if cursor < rows_len { cursor } else { 0 };
    let end = start.saturating_add(budget).min(rows_len);
    // 零预算切不出行：不报 has_more，免得续跑空转。
    let has_more = start < end && end < rows_len;
    (start, end, if has_more { end } else { 0 }, has_more)
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
/// cover_uri 从落地起就带 T002 模板 URL（此前 None——库内 99% 曲目无任何封面
/// 线索，桌面列表只能程序化占位；UI 侧经 CoverGet 盘缓存换本地产物）。
pub(crate) fn qq_track_row(song: &Song) -> hmp_storage::TrackRow {
    // 空名歌手不进展示串/歌手行；mid 缺失时保留 None（交给下轮详情补齐）。
    let singers: Vec<&hmp_qqmusic_api::models::Singer> =
        song.singer.iter().filter(|g| !g.name.is_empty()).collect();
    hmp_storage::TrackRow {
        source: "qq",
        source_key: song.mid.clone(),
        title: song.name.clone(),
        artist: (!singers.is_empty())
            .then(|| hmp_core::join_artist_names(singers.iter().map(|g| g.name.as_str()))),
        artist_refs: singers
            .iter()
            .map(|g| hmp_storage::ArtistRefRow {
                mid: (!g.mid.is_empty()).then(|| g.mid.clone()),
                name: g.name.clone(),
            })
            .collect(),
        album: (!song.album.name.is_empty()).then(|| song.album.name.clone()),
        album_mid: (!song.album.mid.is_empty()).then(|| song.album.mid.clone()),
        duration_ms: (song.interval > 0).then(|| song.interval * 1000),
        cover_uri: cover_uri_from_song(song),
        qq_song_id: (song.id > 0).then_some(song.id),
        ..Default::default()
    }
}

/// Song → 封面 URI（专辑 pmid 套 T002 模板；空 pmid → None，upsert COALESCE
/// 保留既有值）。file:// 已 rebind 的行由 upsert 的 CASE 分支保护不降级。
fn cover_uri_from_song(song: &Song) -> Option<String> {
    let url = crate::content::cover_url_from_pmid(&song.album.pmid);
    (!url.is_empty()).then_some(url)
}

/// 收藏专辑 → relations(album, qq, album_id, liked)。空快照经 `gate` 判定
/// （连续两轮为空才清理）。
async fn reconcile_fav_albums(
    api: &UserApi<'_>,
    euin: &str,
    credential: &Credential,
    library: &Arc<Mutex<LibraryDb>>,
    gate: &mut EmptySnapshotGate,
) {
    let mut page = 1i64;
    let mut present = Vec::new();
    // 快照是否取全：`hasmore=0` 收尾才算完整（同 `reconcile_fav_songs`）。
    let mut complete = false;
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
        if resp.hasmore == 0 {
            complete = true;
            break;
        }
        if page >= 100 || resp.albums.is_empty() {
            break;
        }
        page += 1;
    }
    // 双向：远端已取消收藏的专辑（synced 行）→ desired=0。
    // 只有取全的快照才定缺席；空快照须连续两轮一致才清理。
    if snapshot_allows_cleanup(gate, present.len(), complete, "liked albums")
        && let Ok(mut lib) = library.lock()
    {
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

    // ---- 空快照闸门 / 切片计划（纯函数） ----

    /// D2 回归：空快照首轮只计数不放行，连续两轮为空才放行；非空清零。
    #[test]
    fn empty_snapshot_gate_needs_two_consecutive_empty_rounds() {
        let mut gate = EmptySnapshotGate::default();
        assert!(!gate.observe(0), "首个空快照：只计数");
        assert!(gate.observe(0), "连续两轮为空：放行");
        assert!(gate.observe(0), "持续为空：继续放行");
        assert!(gate.observe(5), "非空：放行");
        assert!(!gate.observe(0), "非空之后的首个空快照：重新计数");
        assert!(gate.observe(0), "再次连续两轮为空：放行");
    }

    /// 切片计划：行数不超预算 → 一片走完，游标归零。
    #[test]
    fn plan_slice_single_pass_resets_cursor() {
        assert_eq!(
            plan_slice(0, 0, PLAYLIST_TRACKS_BUDGET),
            (0, 0, 0, false),
            "无行"
        );
        assert_eq!(plan_slice(3, 0, PLAYLIST_TRACKS_BUDGET), (0, 3, 0, false));
        assert_eq!(
            plan_slice(PLAYLIST_TRACKS_BUDGET, 0, PLAYLIST_TRACKS_BUDGET),
            (0, 24, 0, false)
        );
    }

    /// 切片计划：超过预算 → 分片续跑，走完一遍归零；游标越过行尾从头起算。
    #[test]
    fn plan_slice_walks_rows_then_wraps() {
        assert_eq!(plan_slice(30, 0, 24), (0, 24, 24, true));
        assert_eq!(plan_slice(30, 24, 24), (24, 30, 0, false));
        assert_eq!(plan_slice(60, 24, 24), (24, 48, 48, true));
        assert_eq!(plan_slice(60, 48, 24), (48, 60, 0, false));
        // 行数在本遍中途减少（游标越过行尾）→ 本轮直接从第 0 行起。
        assert_eq!(plan_slice(10, 24, 24), (0, 10, 0, false));
        // 零预算切不出行：不报续跑（免空转）。
        assert_eq!(plan_slice(30, 0, 0), (0, 0, 0, false));
    }

    // ---- wiremock（不触网、不读凭证库；内存库） ----

    /// 指向 wiremock 的客户端 + 测试凭证。
    fn mock_client(server: &wiremock::MockServer) -> (QqMusicClient, Credential) {
        let client = QqMusicClient::with_config(hmp_qqmusic_api::config::ClientConfig {
            base_url: server.uri(),
            ..Default::default()
        });
        let credential = Credential {
            uin: "10001".into(),
            music_id: "10001".into(),
            music_key: "k".into(),
            encrypt_uin: "e-10001".into(),
            ..Default::default()
        };
        (client, credential)
    }

    /// musicu 信封：`req_0.data` 载荷。
    fn envelope(data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "code": 0, "req_0": { "code": 0, "data": data } })
    }

    /// 请求体的 `req_0.param`（解析失败 → Null）。
    fn req_param(req: &wiremock::Request) -> serde_json::Value {
        serde_json::from_slice::<serde_json::Value>(&req.body)
            .map(|b| b["req_0"]["param"].clone())
            .unwrap_or(serde_json::Value::Null)
    }

    /// 「我喜欢」一页快照（`CgiGetDiss` dirid=201）：给定 mid 按序入 songlist。
    fn fav_page(mids: &[&str], hasmore: i64) -> serde_json::Value {
        serde_json::json!({
            "code": 0,
            "songlist": mids
                .iter()
                .enumerate()
                .map(|(i, mid)| serde_json::json!({
                    "id": 1000 + i,
                    "mid": mid,
                    "name": format!("歌{i}"),
                    "type": 1,
                }))
                .collect::<Vec<_>>(),
            "total_song_num": mids.len(),
            "hasmore": hasmore,
        })
    }

    /// 挂「我喜欢」指定页（100 首/页）的响应。
    async fn mount_fav_page(
        server: &wiremock::MockServer,
        song_begin: i64,
        respond: impl wiremock::Respond + 'static,
    ) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/cgi-bin/musicu.fcg"))
            .and(move |req: &wiremock::Request| {
                let param = req_param(req);
                param["dirid"] == serde_json::json!(201)
                    && param["song_begin"] == serde_json::json!(song_begin)
            })
            .respond_with(respond)
            .mount(server)
            .await;
    }

    /// 顺序响应器：`pages` 按请求先后依次返回，用完后重复最后一个。
    fn pages_responder(
        pages: Vec<serde_json::Value>,
    ) -> impl Fn(&wiremock::Request) -> wiremock::ResponseTemplate + Send + Sync + 'static {
        let pages = Arc::new(Mutex::new(pages));
        move |_req| {
            let page = {
                let mut pages = pages.lock().unwrap();
                if pages.len() > 1 {
                    pages.remove(0)
                } else {
                    pages[0].clone()
                }
            };
            wiremock::ResponseTemplate::new(200).set_body_json(envelope(page))
        }
    }

    /// outbox 里的关系行（non-synced）；None = 已 synced。
    fn outbox_row(db: &mut LibraryDb, key: &str) -> Option<hmp_storage::RelationRow> {
        db.relations_pending()
            .unwrap()
            .into_iter()
            .find(|r| r.entity_key == key)
    }

    /// 卡死的 liked 意图行：本地想收藏（desired=1），远端同步一直失败 → error。
    fn seed_stuck_like(db: &mut LibraryDb, mid: &str) {
        db.add_favorite("qq", mid, mid).unwrap();
        db.mark_relation_error("track", "qq", mid, "liked", "QQ Music API error 1101")
            .unwrap();
    }

    /// D2 回归：空快照首轮不清本地；连续第二轮仍为空才允许清理。
    #[tokio::test]
    async fn empty_liked_snapshot_defers_cleanup_until_it_repeats() {
        let server = wiremock::MockServer::start().await;
        mount_fav_page(
            &server,
            0,
            wiremock::ResponseTemplate::new(200).set_body_json(envelope(fav_page(&[], 0))),
        )
        .await;
        let (client, credential) = mock_client(&server);
        let api = UserApi::new(&client);
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        // 本地 synced 行（远端快照在场时 reconcile 写入的形态）。
        lib.lock()
            .unwrap()
            .reconcile_relation("track", "qq", "m-1", "liked", true)
            .unwrap();
        let mut gate = EmptySnapshotGate::default();

        reconcile_fav_songs(&api, "e-10001", &credential, &lib, &mut gate).await;
        assert_eq!(
            lib.lock()
                .unwrap()
                .relation_desired("track", "qq", "m-1", "liked")
                .unwrap(),
            Some(true),
            "首个空快照不得清本地"
        );

        reconcile_fav_songs(&api, "e-10001", &credential, &lib, &mut gate).await;
        assert_eq!(
            lib.lock()
                .unwrap()
                .relation_desired("track", "qq", "m-1", "liked")
                .unwrap(),
            Some(false),
            "连续两轮为空 → 权威空，清理放行"
        );
    }

    /// D2 回归：中间穿插非空快照 → 计数清零，其后的首个空快照仍不清本地。
    #[tokio::test]
    async fn nonempty_liked_snapshot_resets_empty_gate() {
        let server = wiremock::MockServer::start().await;
        mount_fav_page(
            &server,
            0,
            pages_responder(vec![
                fav_page(&[], 0),
                fav_page(&["m-1", "m-2"], 0),
                fav_page(&[], 0),
            ]),
        )
        .await;
        let (client, credential) = mock_client(&server);
        let api = UserApi::new(&client);
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        lib.lock()
            .unwrap()
            .reconcile_relation("track", "qq", "m-1", "liked", true)
            .unwrap();
        let mut gate = EmptySnapshotGate::default();

        for _ in 0..3 {
            reconcile_fav_songs(&api, "e-10001", &credential, &lib, &mut gate).await;
        }
        let mut db = lib.lock().unwrap();
        for mid in ["m-1", "m-2"] {
            assert_eq!(
                db.relation_desired("track", "qq", mid, "liked").unwrap(),
                Some(true),
                "{mid}: 非空快照后的首个空快照不得清本地"
            );
        }
    }

    /// 完整快照后收口：在场 + desired=1 的 error 行 → synced；缺席的 error 行
    /// 与「在场但意图为取消」的行不动。
    #[tokio::test]
    async fn present_liked_error_row_closes_after_full_snapshot() {
        let server = wiremock::MockServer::start().await;
        mount_fav_page(
            &server,
            0,
            wiremock::ResponseTemplate::new(200)
                .set_body_json(envelope(fav_page(&["m-stuck", "m-unlike", "m-plain"], 0))),
        )
        .await;
        let (client, credential) = mock_client(&server);
        let api = UserApi::new(&client);
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        {
            let mut db = lib.lock().unwrap();
            seed_stuck_like(&mut db, "m-stuck");
            // 对照 1：同样卡死但远端缺席 → 意图未必已落实，不动。
            seed_stuck_like(&mut db, "m-away");
            // 对照 2：在场但本地意图是取消收藏 → 远端尚未反映，不动。
            db.set_relation("track", "qq", "m-unlike", "liked", false)
                .unwrap();
            db.mark_relation_error("track", "qq", "m-unlike", "liked", "x")
                .unwrap();
        }

        reconcile_fav_songs(
            &api,
            "e-10001",
            &credential,
            &lib,
            &mut EmptySnapshotGate::default(),
        )
        .await;

        let mut db = lib.lock().unwrap();
        assert!(
            outbox_row(&mut db, "m-stuck").is_none(),
            "在场 + desired=1 的 error 行应收口为 synced（不再进 outbox）"
        );
        assert_eq!(
            db.relation_desired("track", "qq", "m-stuck", "liked")
                .unwrap(),
            Some(true)
        );
        assert!(
            outbox_row(&mut db, "m-away").is_some_and(|r| r.desired_state),
            "缺席的 error 行不动"
        );
        assert!(
            outbox_row(&mut db, "m-unlike").is_some_and(|r| !r.desired_state),
            "在场但意图为取消的行不动"
        );
        assert_eq!(
            db.relation_desired("track", "qq", "m-plain", "liked")
                .unwrap(),
            Some(true)
        );
    }

    /// 快照不完整（翻页失败）→ 不收口：在场曲目只能证明远端有它，但「完整」
    /// 是收口的前提。
    #[tokio::test]
    async fn liked_error_row_stays_when_snapshot_incomplete() {
        let server = wiremock::MockServer::start().await;
        // 第 1 页正常（hasmore=1）；第 2 页 HTTP 500 → 快照不完整。
        mount_fav_page(&server, 100, wiremock::ResponseTemplate::new(500)).await;
        mount_fav_page(
            &server,
            0,
            wiremock::ResponseTemplate::new(200).set_body_json(envelope(fav_page(&["m-stuck"], 1))),
        )
        .await;
        let (client, credential) = mock_client(&server);
        let api = UserApi::new(&client);
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        seed_stuck_like(&mut lib.lock().unwrap(), "m-stuck");

        reconcile_fav_songs(
            &api,
            "e-10001",
            &credential,
            &lib,
            &mut EmptySnapshotGate::default(),
        )
        .await;

        assert!(
            outbox_row(&mut lib.lock().unwrap(), "m-stuck")
                .is_some_and(|r| r.sync_state == "error"),
            "快照不完整 → error 行不收口"
        );
    }

    /// 半份快照不定缺席：第 2 页软失败返回空页（`hasmore=1` 但列表为空）→
    /// 翻页提前止，`present` 只有第 1 页——此前照样按它清理，第 1 页之外的
    /// 已同步喜欢全被置 0（与 §30.1 的分片清理同一类症状）。
    #[tokio::test]
    async fn incomplete_liked_snapshot_keeps_absent_rows() {
        let server = wiremock::MockServer::start().await;
        mount_fav_page(
            &server,
            0,
            wiremock::ResponseTemplate::new(200).set_body_json(envelope(fav_page(&["m-1"], 1))),
        )
        .await;
        mount_fav_page(
            &server,
            100,
            wiremock::ResponseTemplate::new(200).set_body_json(envelope(fav_page(&[], 1))),
        )
        .await;
        let (client, credential) = mock_client(&server);
        let api = UserApi::new(&client);
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        {
            let mut db = lib.lock().unwrap();
            for mid in ["m-1", "m-2", "m-3"] {
                db.reconcile_relation("track", "qq", mid, "liked", true)
                    .unwrap();
            }
        }

        reconcile_fav_songs(
            &api,
            "e-10001",
            &credential,
            &lib,
            &mut EmptySnapshotGate::default(),
        )
        .await;

        let mut db = lib.lock().unwrap();
        for mid in ["m-1", "m-2", "m-3"] {
            assert_eq!(
                db.relation_desired("track", "qq", mid, "liked").unwrap(),
                Some(true),
                "{mid}: 半份快照不得把没拉到的曲目当缺席清掉"
            );
        }
    }

    /// 收藏歌单一页快照（`CgiGetPlaylistFavInfo`）：列表键 `v_list`，元素键
    /// `tid`/`name`（线上键名）。
    fn fav_songlist_page(ids: &[i64], hasmore: i64) -> serde_json::Value {
        serde_json::json!({
            "number": ids.len(),
            "hasmore": hasmore,
            "total": ids.len(),
            "v_list": ids
                .iter()
                .map(|id| serde_json::json!({"tid": id, "name": "歌单", "songnum": 1}))
                .collect::<Vec<_>>(),
        })
    }

    /// 挂收藏歌单指定偏移（100 个/页）的响应。
    async fn mount_fav_songlist_offset(
        server: &wiremock::MockServer,
        offset: i64,
        body: serde_json::Value,
    ) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/cgi-bin/musicu.fcg"))
            .and(move |req: &wiremock::Request| {
                req_param(req)["offset"] == serde_json::json!(offset)
            })
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(envelope(body)))
            .mount(server)
            .await;
    }

    /// 半份快照不定缺席（订阅歌单）：清理会连带删掉本地歌单行与曲目缓存，
    /// 比「我喜欢」更具破坏性——第 2 页空页时两份本地订阅歌单都必须还在。
    #[tokio::test]
    async fn incomplete_subscribed_snapshot_keeps_playlists() {
        let server = wiremock::MockServer::start().await;
        mount_fav_songlist_offset(&server, 0, fav_songlist_page(&[7001], 1)).await;
        mount_fav_songlist_offset(&server, 100, fav_songlist_page(&[], 1)).await;
        let (client, credential) = mock_client(&server);
        let api = UserApi::new(&client);
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        {
            let mut db = lib.lock().unwrap();
            for id in ["7001", "7002"] {
                db.reconcile_relation("playlist", "qq", id, "subscribed", true)
                    .unwrap();
                db.reconcile_playlist(id, "歌单", "subscribed").unwrap();
            }
        }

        reconcile_fav_songlists(
            &api,
            "e-10001",
            &credential,
            &lib,
            &mut EmptySnapshotGate::default(),
        )
        .await;

        let mut db = lib.lock().unwrap();
        let remote_ids: Vec<String> = db
            .list_playlists()
            .unwrap()
            .into_iter()
            .filter_map(|p| p.remote_id)
            .collect();
        assert_eq!(remote_ids.len(), 2, "半份快照不得删掉没拉到的订阅歌单");
        for id in ["7001", "7002"] {
            assert_eq!(
                db.relation_desired("playlist", "qq", id, "subscribed")
                    .unwrap(),
                Some(true),
                "{id}: 订阅关系保持"
            );
        }
    }

    /// 订阅歌单快照（`CgiGetDiss`，disstid > 0）：1 首，mid 随 disstid 派生。
    fn sub_snapshot(req: &wiremock::Request) -> wiremock::ResponseTemplate {
        let mid = format!("m-{}", req_param(req)["disstid"]);
        wiremock::ResponseTemplate::new(200).set_body_json(envelope(serde_json::json!({
            "code": 0,
            "dirinfo": {"dissid": 0, "dissname": "快照歌单", "songnum": 1},
            "songlist": [{"id": 1, "mid": mid, "name": "歌", "type": 1}],
            "songlist_size": 1,
            "total_song_num": 1,
            "hasmore": 0,
        })))
    }

    /// D5 回归：歌单超过预算 → 首轮只走前 24 行、游标停在 24；续跑片走完余下
    /// 行后游标归零；再跑一遍从头开始、同快照零变更。
    #[tokio::test]
    async fn playlist_tracks_cursor_walks_all_rows_across_slices() {
        let total = PLAYLIST_TRACKS_BUDGET + 6; // 超过单轮预算
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/cgi-bin/musicu.fcg"))
            .and(|req: &wiremock::Request| {
                req_param(req)["disstid"]
                    .as_i64()
                    .is_some_and(|tid| tid > 0)
            })
            .respond_with(sub_snapshot)
            .mount(&server)
            .await;
        let (client, credential) = mock_client(&server);
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let order: Vec<i64> = {
            let mut db = lib.lock().unwrap();
            (1..=total as i64)
                .map(|tid| {
                    db.reconcile_playlist(&tid.to_string(), "订阅单", "subscribed")
                        .unwrap()
                })
                .collect()
        };
        let track_counts = |db: &mut LibraryDb| -> Vec<i64> {
            let counts: Vec<i64> = order.iter().map(|id| track_count(db, *id)).collect();
            counts
        };
        let mut state = ReconcileState::default();

        // 首轮：预算 24 → 只走前 24 行，游标停在 24，待续跑。
        let (changed, has_more) =
            reconcile_playlist_tracks(&client, &credential, &lib, &[], &mut state.tracks_cursor)
                .await;
        assert_eq!((changed, has_more), (PLAYLIST_TRACKS_BUDGET, true));
        assert_eq!(state.tracks_cursor, PLAYLIST_TRACKS_BUDGET);
        assert_eq!(snapshot_requests(&server).await, PLAYLIST_TRACKS_BUDGET);
        {
            let mut db = lib.lock().unwrap();
            let counts = track_counts(&mut db);
            assert!(
                counts[..PLAYLIST_TRACKS_BUDGET].iter().all(|c| *c == 1),
                "前 24 个歌单补到曲目"
            );
            assert!(
                counts[PLAYLIST_TRACKS_BUDGET..].iter().all(|c| *c == 0),
                "排在预算之后的歌单本轮未处理"
            );
        }

        // 续跑片：走完余下 6 行 → 本遍结束，游标归零。
        let (changed, has_more) =
            resume_playlist_tracks(&client, &credential, &lib, &mut state).await;
        assert_eq!((changed, has_more), (6, false));
        assert_eq!(state.tracks_cursor, 0);

        // 再跑一遍：从头开始（下标 0 起 24 行），同快照零变更。
        let (changed, has_more) =
            reconcile_playlist_tracks(&client, &credential, &lib, &[], &mut state.tracks_cursor)
                .await;
        assert_eq!((changed, has_more), (0, true));
        assert_eq!(state.tracks_cursor, PLAYLIST_TRACKS_BUDGET);
        assert_eq!(
            snapshot_requests(&server).await,
            PLAYLIST_TRACKS_BUDGET * 2 + 6
        );
        let mut db = lib.lock().unwrap();
        assert!(
            track_counts(&mut db).iter().all(|c| *c == 1),
            "全部 {total} 个歌单都补到曲目"
        );
    }

    /// 歌单曲目数。
    fn track_count(db: &mut LibraryDb, playlist_id: i64) -> i64 {
        db.list_playlists()
            .unwrap()
            .into_iter()
            .find(|p| p.id == playlist_id)
            .map(|p| p.track_count)
            .expect("歌单行应存在")
    }

    /// 收到的歌单快照请求数（`CgiGetDiss`，disstid > 0）。
    async fn snapshot_requests(server: &wiremock::MockServer) -> usize {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|req| {
                req_param(req)["disstid"]
                    .as_i64()
                    .is_some_and(|tid| tid > 0)
            })
            .count()
    }

    /// 构造带歌手/专辑 mid 的 QQ 曲目（phase 1 身份字段的输入形态）。
    fn song_with_mids() -> Song {
        use hmp_qqmusic_api::models::{Album, Singer};
        Song {
            id: 42,
            mid: "mid-qq".to_owned(),
            name: "希望有羽毛和翅膀".to_owned(),
            interval: 180,
            singer: vec![
                Singer {
                    id: 1,
                    mid: "singer-a".to_owned(),
                    name: "知更鸟".to_owned(),
                    ..Default::default()
                },
                Singer {
                    id: 2,
                    mid: "singer-b".to_owned(),
                    name: "HOYO-MiX".to_owned(),
                    ..Default::default()
                },
                Singer {
                    id: 3,
                    mid: "singer-c".to_owned(),
                    name: "Chevy".to_owned(),
                    ..Default::default()
                },
            ],
            album: Album {
                id: 7,
                mid: "album-mid".to_owned(),
                name: "原神-闪耀的群星".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// §30.2.6 phase 1：QQ 入库保留 singer[].mid 与 album.mid（此前入库即丢）。
    #[test]
    fn qq_ingest_keeps_artist_and_album_mids() {
        use hmp_storage::{ArtistRefRow, LibraryDb};
        let mut db = LibraryDb::open_in_memory().unwrap();
        let tid = db.upsert_track(&qq_track_row(&song_with_mids())).unwrap();
        assert_eq!(
            db.album_mid(tid).unwrap().as_deref(),
            Some("album-mid"),
            "专辑 mid 落库"
        );
        assert_eq!(
            db.track_artists(tid).unwrap(),
            vec![
                ArtistRefRow {
                    mid: Some("singer-a".into()),
                    name: "知更鸟".into()
                },
                ArtistRefRow {
                    mid: Some("singer-b".into()),
                    name: "HOYO-MiX".into()
                },
                ArtistRefRow {
                    mid: Some("singer-c".into()),
                    name: "Chevy".into()
                },
            ],
            "歌手 mid 按 position 落库"
        );
        let meta = db.track_meta_batch("qq", &["mid-qq".to_owned()]).unwrap();
        assert_eq!(
            meta[0].artist.as_deref(),
            Some("知更鸟 / HOYO-MiX / Chevy"),
            "展示串统一为 \" / \""
        );
    }

    /// §30.2.6 phase 1：同一组歌手经 qq 入库 / stub 缓存 / 播放详情三条生产
    /// 路径产出的 `artist` 展示串完全一致（不再混用 ", " 与标签原文）。
    #[test]
    fn artist_display_string_is_identical_across_producers() {
        use hmp_core::{AlbumRef, ArtistId, ArtistRef, Track, TrackId};
        let song = song_with_mids();
        let names = ["知更鸟", "HOYO-MiX", "Chevy"];
        let qq = qq_track_row(&song);
        let stub = crate::engine::stub_row(&hmp_core::TrackStub {
            id: TrackId::new(song.mid.clone()),
            title: song.name.clone(),
            artists: names.iter().map(|n| (*n).to_owned()).collect(),
            artist_refs: ["singer-a", "singer-b", "singer-c"]
                .iter()
                .zip(names)
                .map(|(mid, name)| ArtistRef {
                    id: ArtistId::new(*mid),
                    name: (*name).to_owned(),
                })
                .collect(),
            album: Some(song.album.name.clone()),
            album_mid: Some(song.album.mid.clone()),
            duration_ms: Some(180_000),
        });
        let mut track = Track::new(TrackId::new(song.mid.clone()), song.name.clone());
        track.artists = ["singer-a", "singer-b", "singer-c"]
            .iter()
            .zip(names)
            .map(|(mid, name)| ArtistRef {
                id: ArtistId::new(*mid),
                name: (*name).to_owned(),
            })
            .collect();
        track.album = Some(AlbumRef {
            id: hmp_core::AlbumId::new(song.album.mid.clone()),
            name: song.album.name.clone(),
        });
        let play = crate::engine::track_row(&track);
        assert_eq!(qq.artist.as_deref(), Some("知更鸟 / HOYO-MiX / Chevy"));
        assert_eq!(stub.artist, qq.artist, "stub 缓存与入库同口径");
        assert_eq!(play.artist, qq.artist, "播放详情与入库同口径");
        // mid 也随三条路径一致保留（本地行才允许 NULL）。
        assert_eq!(qq.album_mid.as_deref(), Some("album-mid"));
        assert_eq!(stub.album_mid, qq.album_mid);
        assert_eq!(play.album_mid, qq.album_mid);
        assert_eq!(stub.artist_refs, qq.artist_refs);
    }
}
