//! QQ 乐观同步 worker（本地先提交、远端趋同；spec §3.2）。
//!
//! 消费三张 durable outbox：`relations`（收藏/订阅，含操作合并后的最终意图）、
//! `playlists`（owned 歌单创建/删除）、`playlist_ops`（owned 歌单曲目增删）。
//! 无凭证时整体跳过（离线意图合法留存）；失败指数退避重试
//! （`min(2^retry_count * 10s, 10min)`，由行内 retry_count 驱动）。
//!
//! 触发：写命令经 [`SyncHandle::trigger`] 通知；30s 定时兜底扫描。
//! 歌单曲目补抓一轮走不完时，由内部定时分支短延时续跑
//! （`SyncMsg::ResumeTracks`，不占通道、不阻塞其它同步消息）。

use std::sync::{Arc, Mutex};

use hmp_qqmusic_api::{
    QqMusicClient, album::AlbumApi, credential::Credential, song::SongApi, songlist::SonglistApi,
};
use hmp_storage::{LibraryDb, RelationRow};
use tokio::sync::mpsc;

use crate::reconcile::ReconcileState;

/// 歌单曲目补抓续跑延时（AUDIT §30 D5）：本轮预算用完且本遍未走完时，先
/// 交还控制权（不阻塞其它同步消息），隔此延时再续跑下一片。
const TRACKS_RESUME_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// worker 消息。
#[derive(Clone, Copy)]
enum SyncMsg {
    /// 消费 outbox（写命令落地后触发）。
    Sync,
    /// 从 QQ 拉用户库快照 reconcile（`hmp library sync`）。
    Reconcile,
    /// 歌单曲目补抓续跑片（仅由 worker 内部定时产生，不经通道）。
    ResumeTracks,
}

/// 同步 worker 句柄（server 持有；写命令落地后触发）。
#[derive(Clone)]
pub struct SyncHandle {
    notify: mpsc::UnboundedSender<SyncMsg>,
}

impl SyncHandle {
    /// 触发一次 outbox 同步（无阻塞；worker 合并通知）。
    pub fn trigger(&self) {
        let _ = self.notify.send(SyncMsg::Sync);
    }

    /// 触发 QQ 用户库 reconcile（本地意图胜出规则见 spec §3.1）。
    pub fn reconcile(&self) {
        let _ = self.notify.send(SyncMsg::Reconcile);
    }
}

/// 后台同步 worker。
pub struct SyncWorker {
    library: Arc<Mutex<LibraryDb>>,
    client: QqMusicClient,
    store: Box<dyn hmp_storage::credential::CredentialStore>,
    notify: mpsc::UnboundedReceiver<SyncMsg>,
    /// 库变更代际（reconcile 落库后 bump，server 推 `LibraryChanged`）。
    library_changed: tokio::sync::watch::Sender<u64>,
    /// reconcile 跨轮状态（空快照闸门 + 歌单曲目补抓游标；仅内存，重启清零）。
    reconcile_state: ReconcileState,
    /// 歌单曲目补抓续跑时刻：本遍未走完时存在，走完即撤销。
    tracks_resume_at: Option<tokio::time::Instant>,
}

/// 单次重试间隔：`min(2^retry * 10s, 10min)`。
fn retry_delay(retry_count: i64) -> std::time::Duration {
    let secs = 10i64.saturating_mul(1i64 << retry_count.min(6));
    std::time::Duration::from_secs(secs.min(600) as u64)
}

/// 等到续跑时刻；无续跑时刻则永不就绪（select 分支闲置）。
async fn wait_tracks_resume(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

impl SyncWorker {
    /// 启动 worker，返回触发句柄。
    pub fn spawn(
        library: Arc<Mutex<LibraryDb>>,
        client: QqMusicClient,
        store: Box<dyn hmp_storage::credential::CredentialStore>,
        library_changed: tokio::sync::watch::Sender<u64>,
    ) -> SyncHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut worker = Self {
            library,
            client,
            store,
            notify: rx,
            library_changed,
            reconcile_state: ReconcileState::default(),
            tracks_resume_at: None,
        };
        tokio::spawn(async move { worker.run().await });
        SyncHandle { notify: tx }
    }

    async fn run(&mut self) {
        loop {
            let msg = tokio::select! {
                msg = self.notify.recv() => msg,
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => Some(SyncMsg::Sync),
                _ = wait_tracks_resume(self.tracks_resume_at) => Some(SyncMsg::ResumeTracks),
            };
            match msg {
                Some(SyncMsg::Sync) => {
                    if self.sync_once().await {
                        self.library_changed.send_modify(|g| *g += 1);
                    }
                }
                Some(SyncMsg::Reconcile) => {
                    // 先消费 outbox（推送完成再拉快照）：避免「已 synced 但快照旧 →
                    // 缺席误翻」的最终一致闪烁窗口。
                    if self.sync_once().await {
                        self.library_changed.send_modify(|g| *g += 1);
                    }
                    self.reconcile().await;
                }
                Some(SyncMsg::ResumeTracks) => {
                    // 续跑片：只跑歌单曲目补抓（其余快照源本遍已拉过）。先撤销旧
                    // 时刻，由本轮结果决定是否再排下一片；曲目缓存有变更才推库变更。
                    self.tracks_resume_at = None;
                    if self.resume_playlist_tracks().await {
                        self.library_changed.send_modify(|g| *g += 1);
                    }
                }
                None => return, // 通道关闭（daemon 退出）
            }
        }
    }

    /// QQ 用户库快照 reconcile（Phase 4；未登录 → 跳过）。
    async fn reconcile(&mut self) {
        let Some(credential) = self.load_credential() else {
            return;
        };
        let has_more = crate::reconcile::reconcile_user_library(
            &self.client,
            &credential,
            &self.library,
            &mut self.reconcile_state,
        )
        .await;
        // 快照落库 → 媒体库内容变更（客户端刷新歌单/我喜欢页，AUDIT §8.9）。
        self.library_changed.send_modify(|g| *g += 1);
        self.schedule_tracks_resume(has_more);
    }

    /// 歌单曲目补抓续跑片；返回曲目缓存是否有变更。
    async fn resume_playlist_tracks(&mut self) -> bool {
        let Some(credential) = self.load_credential() else {
            return false; // 未登录：本遍作废，下次 reconcile 触发再续
        };
        let (changed, has_more) = crate::reconcile::resume_playlist_tracks(
            &self.client,
            &credential,
            &self.library,
            &mut self.reconcile_state,
        )
        .await;
        self.schedule_tracks_resume(has_more);
        changed > 0
    }

    /// 本遍未走完 → 短延时后自动续跑下一片；走完 → 不再续跑。
    fn schedule_tracks_resume(&mut self, has_more: bool) {
        self.tracks_resume_at = has_more.then(|| tokio::time::Instant::now() + TRACKS_RESUME_DELAY);
    }

    /// 一轮同步：relations → playlists（owned）→ playlist_ops。
    /// 返回是否有行被消费（状态标记/本地行变更 → 媒体库内容变更）。
    async fn sync_once(&mut self) -> bool {
        let Some(credential) = self.load_credential() else {
            return false; // 无凭证：离线意图留 outbox（合法状态）
        };
        let mut changed = false;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut relations = {
            let Ok(mut lib) = self.library.lock() else {
                return changed;
            };
            lib.relations_pending().unwrap_or_default()
        };
        // 指数退避：error 行在上次尝试后的退避窗口内不重试。
        relations.retain(|r| {
            r.sync_state == "pending"
                || now - r.updated_at >= retry_delay(r.retry_count).as_secs() as i64
        });
        changed |= !relations.is_empty();
        for row in relations {
            self.sync_relation(&row, &credential).await;
        }
        let playlists = {
            let Ok(mut lib) = self.library.lock() else {
                return changed;
            };
            lib.playlists_pending().unwrap_or_default()
        };
        // 指数退避：error 行在退避窗口内不重试（与 relations 一致）。
        let mut playlists = playlists;
        playlists.retain(|r| {
            r.sync_state == "pending"
                || now - r.updated_at.unwrap_or(0) >= retry_delay(r.retry_count).as_secs() as i64
        });
        changed |= !playlists.is_empty();
        for row in playlists {
            self.sync_playlist(&row, &credential).await;
        }
        let ops = {
            let Ok(mut lib) = self.library.lock() else {
                return changed;
            };
            lib.playlist_ops_pending().unwrap_or_default()
        };
        let mut ops = ops;
        ops.retain(|o| {
            o.sync_state == "pending"
                || now - o.updated_at.unwrap_or(0) >= retry_delay(o.retry_count).as_secs() as i64
        });
        changed |= !ops.is_empty();
        for op in ops {
            self.sync_playlist_op(&op, &credential).await;
        }
        changed
    }

    fn load_credential(&self) -> Option<Credential> {
        self.store
            .load()
            .ok()
            .flatten()
            .filter(|c| c.is_logged_in())
    }

    /// 关系行同步（track/liked、playlist/subscribed、album/liked）。
    async fn sync_relation(&mut self, row: &RelationRow, credential: &Credential) {
        let key = row.entity_key.clone();
        let result = match (row.entity_type.as_str(), row.relation.as_str()) {
            ("track", "liked") => {
                // numeric song id：先查库（列表解析缓存），缺失则详情补全；仍缺 → 跳过等待。
                let song_id = self.resolve_song_id(&key).await;
                match song_id {
                    Some(sid) => {
                        let api = SonglistApi::new(&self.client);
                        if row.desired_state {
                            api.like_song(&[(sid, 1)], credential).await
                        } else {
                            api.unlike_song(&[(sid, 1)], credential).await
                        }
                        .map(|_| ())
                    }
                    None => return self.fail_relation(row, "cannot parse QQ numeric song id"),
                }
            }
            ("playlist", "subscribed") => {
                let disstid: i64 = match key.parse() {
                    Ok(v) => v,
                    Err(_) => {
                        return self.fail_relation(row, "playlist id is not numeric");
                    }
                };
                let api = hmp_qqmusic_api::UserApi::new(&self.client);
                if row.desired_state {
                    api.fav_songlist(disstid, credential).await
                } else {
                    api.unfav_songlist(disstid, credential).await
                }
                .map(|_| ())
            }
            ("album", "liked") => {
                let album_id: i64 = match key.parse() {
                    Ok(v) => v,
                    Err(_) => {
                        return self.fail_relation(row, "album id is not numeric");
                    }
                };
                let api = AlbumApi::new(&self.client);
                if row.desired_state {
                    api.fav_album(&[album_id], credential).await.map(|_| ())
                } else {
                    api.del_fav_album(&[album_id], credential).await.map(|_| ())
                }
            }
            _ => return, // 未知实体/关系：忽略（不阻塞其他行）
        };
        match result {
            Ok(()) => self.mark_relation_ok(row),
            Err(e) => self.fail_relation(row, &e.to_string()),
        }
    }

    /// owned 歌单本体同步：仅处理「本地新建待回填 dirid」的行；
    /// 删除意图由 `playlist_ops` 的 `delete_playlist` 操作驱动（行保留到远端删除成功）。
    async fn sync_playlist(&mut self, row: &hmp_storage::PlaylistRow, credential: &Credential) {
        if row.remote_id.is_some() {
            return; // 有远端身份的行不在此处理（删除由 playlist_ops 驱动）
        }
        // 新创建（本地先提交）：调 QQ 创建，回填 dirid。
        let api = SonglistApi::new(&self.client);
        let result = match api.create(&row.name, credential).await {
            Ok(resp) if resp.ret_code == 0 && resp.dirid > 0 => {
                let Ok(mut lib) = self.library.lock() else {
                    return;
                };
                match lib.set_playlist_remote(row.id, &resp.dirid.to_string(), "owned") {
                    Ok(()) => Ok(()),
                    Err(e) => Err(e.to_string()),
                }
            }
            Ok(resp) => Err(format!(
                "failed to create QQ playlist: ret_code={}",
                resp.ret_code
            )),
            Err(e) => Err(e.to_string()),
        };
        let Ok(mut lib) = self.library.lock() else {
            return;
        };
        match result {
            Ok(()) => {
                let _ = lib.mark_playlist_synced(row.id);
            }
            Err(e) => {
                let _ = lib.mark_playlist_error(row.id, &e);
            }
        }
    }

    /// owned 歌单操作同步：`add`/`del`（曲目增删）与 `delete_playlist`（远端删除）。
    async fn sync_playlist_op(&mut self, op: &hmp_storage::PlaylistOpRow, credential: &Credential) {
        let mut dirid = {
            let Ok(mut lib) = self.library.lock() else {
                return;
            };
            lib.playlist_remote_id(op.playlist_id)
                .ok()
                .flatten()
                .and_then(|d| d.parse::<i64>().ok())
        };
        if op.op != "delete_playlist" {
            let Some(d) = dirid else {
                return; // 歌单尚无远端身份：等 sync_playlist 创建后再试
            };
            dirid = Some(d);
        }
        let api = SonglistApi::new(&self.client);
        let result = match op.op.as_str() {
            "add" | "del" => {
                // numeric id：op 行 → mid 详情补全（写回）→ 再同步。
                let song_id = match op.song_id {
                    Some(v) => Some(v),
                    None => match &op.song_key {
                        Some(mid) => {
                            let id = self.resolve_song_id(mid).await;
                            if let (Some(id), Ok(mut lib)) = (id, self.library.lock()) {
                                let _ = lib.set_op_song_id(op.id, id);
                            }
                            id
                        }
                        None => None,
                    },
                };
                let Some(d) = dirid else { return };
                let Some(song_id) = song_id else {
                    // numeric id 未知：标记错误（退避重试），不再无限 pending。
                    let Ok(mut lib) = self.library.lock() else {
                        return;
                    };
                    let _ = lib.mark_op_error(op.id, "cannot parse QQ numeric song id");
                    return;
                };
                let api = SonglistApi::new(&self.client);
                if op.op == "add" {
                    api.add_songs(d, &[(song_id, 1)], 0, credential).await
                } else {
                    api.del_songs(d, &[(song_id, 1)], 0, credential).await
                }
                .map(|_| ())
                .map_err(|e| e.to_string())
            }
            "delete_playlist" => {
                // 远端删除成功后才删本地行（含 op 行）。
                let Some(d) = dirid else {
                    return;
                };
                match api.delete(d, credential).await {
                    Ok(resp) if resp.ret_code == 0 => {
                        let Ok(mut lib) = self.library.lock() else {
                            return;
                        };
                        let _ = lib.delete_playlist(op.playlist_id);
                        let _ = lib.mark_op_done(op.id);
                        return;
                    }
                    Ok(resp) => Err(format!(
                        "failed to delete QQ playlist: ret_code={}",
                        resp.ret_code
                    )),
                    Err(e) => Err(e.to_string()),
                }
            }
            _ => return,
        };
        let Ok(mut lib) = self.library.lock() else {
            return;
        };
        match result {
            Ok(()) => {
                let _ = lib.mark_op_done(op.id);
            }
            Err(e) => {
                let _ = lib.mark_op_error(op.id, &e.to_string());
            }
        }
    }

    /// QQ numeric song id：库缓存 → 详情补全（写回库）。
    async fn resolve_song_id(&self, mid: &str) -> Option<i64> {
        {
            let Ok(mut lib) = self.library.lock() else {
                return None;
            };
            if let Ok(Some(v)) = lib.qq_song_id("qq", mid) {
                return Some(v);
            }
        }
        let api = SongApi::new(&self.client);
        let id = api
            .get_detail(mid)
            .await
            .ok()
            .map(|resp| resp.track.id)
            .filter(|id| *id > 0)?;
        if let Ok(mut lib) = self.library.lock() {
            let _ = lib.set_track_qq_song_id("qq", mid, id);
        }
        Some(id)
    }

    fn mark_relation_ok(&self, row: &RelationRow) {
        if let Ok(mut lib) = self.library.lock() {
            let _ = lib.mark_relation_synced(
                &row.entity_type,
                &row.provider,
                &row.entity_key,
                &row.relation,
            );
        }
    }

    fn fail_relation(&self, row: &RelationRow, err: &str) {
        if let Ok(mut lib) = self.library.lock() {
            let _ = lib.mark_relation_error(
                &row.entity_type,
                &row.provider,
                &row.entity_key,
                &row.relation,
                err,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_backs_off_exponentially() {
        assert_eq!(retry_delay(0), std::time::Duration::from_secs(10));
        assert_eq!(retry_delay(2), std::time::Duration::from_secs(40));
        assert_eq!(retry_delay(5), std::time::Duration::from_secs(320));
        assert_eq!(
            retry_delay(10),
            std::time::Duration::from_secs(600),
            "封顶 10min"
        );
    }

    /// 固定凭证存储（测试用：恒返回同一已登录凭证，不触盘、不碰密钥环）。
    struct FixedStore(Credential);

    impl hmp_storage::credential::CredentialStore for FixedStore {
        fn save(&self, _credential: &Credential) -> Result<(), hmp_core::HmpError> {
            Ok(())
        }
        fn load(&self) -> Result<Option<Credential>, hmp_core::HmpError> {
            Ok(Some(self.0.clone()))
        }
        fn delete(&self) -> Result<(), hmp_core::HmpError> {
            Ok(())
        }
    }

    /// 测试 worker：不启动 run 循环，直接驱动各处理函数；返回库句柄与库变更接收端。
    fn test_worker(
        server: &wiremock::MockServer,
    ) -> (
        SyncWorker,
        Arc<Mutex<LibraryDb>>,
        tokio::sync::watch::Receiver<u64>,
    ) {
        let library = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let (_tx, rx) = mpsc::unbounded_channel();
        let (library_changed, changed_rx) = tokio::sync::watch::channel(0u64);
        let credential = Credential {
            uin: "10001".into(),
            music_id: "10001".into(),
            music_key: "k".into(),
            encrypt_uin: "e-10001".into(),
            ..Default::default()
        };
        let client = QqMusicClient::with_config(hmp_qqmusic_api::config::ClientConfig {
            base_url: server.uri(),
            ..Default::default()
        });
        let worker = SyncWorker {
            library: library.clone(),
            client,
            store: Box::new(FixedStore(credential)),
            notify: rx,
            library_changed,
            reconcile_state: ReconcileState::default(),
            tracks_resume_at: None,
        };
        (worker, library, changed_rx)
    }

    /// 无续跑时刻 → 定时分支永不就绪（不空转唤醒）；有 → 到点即绪。
    #[tokio::test]
    async fn tracks_resume_wait_is_idle_without_a_deadline() {
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                wait_tracks_resume(None)
            )
            .await
            .is_err(),
            "无续跑时刻不应就绪"
        );
        let at = tokio::time::Instant::now();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_tracks_resume(Some(at)),
        )
        .await
        .expect("到点的续跑时刻应就绪");
    }

    /// D5 回归：首轮 reconcile 只走预算内（24）的歌单 → 续跑时刻排在短延时后；
    /// 续跑片走完余下歌单 → 撤销续跑、有变更才推库变更；再触发则从头分片。
    #[tokio::test]
    async fn worker_resumes_playlist_tracks_until_the_pass_completes() {
        let server = wiremock::MockServer::start().await;
        // 歌单快照：每个 disstid 回 1 首（mid 随 disstid 派生）。其余接口不挂 mock
        // （404 → 各快照源本轮跳过），只剩曲目补抓在干活。
        let snapshot = |req: &wiremock::Request| {
            let param =
                serde_json::from_slice::<serde_json::Value>(&req.body).unwrap()["req_0"]["param"]
                    .clone();
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "req_0": {"code": 0, "data": {
                    "code": 0,
                    "dirinfo": {"dissid": 0, "dissname": "快照歌单", "songnum": 1},
                    "songlist": [{"id": 1, "mid": format!("m-{}", param["disstid"]), "name": "歌", "type": 1}],
                    "songlist_size": 1,
                    "total_song_num": 1,
                    "hasmore": 0,
                }}
            }))
        };
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/cgi-bin/musicu.fcg"))
            .and(|req: &wiremock::Request| {
                serde_json::from_slice::<serde_json::Value>(&req.body)
                    .map(|b| {
                        b["req_0"]["param"]["disstid"]
                            .as_i64()
                            .is_some_and(|t| t > 0)
                    })
                    .unwrap_or(false)
            })
            .respond_with(snapshot)
            .mount(&server)
            .await;
        let (mut worker, library, mut changed_rx) = test_worker(&server);
        let total = 30i64; // 超过单轮预算 24
        {
            let mut db = library.lock().unwrap();
            for tid in 1..=total {
                db.reconcile_playlist(&tid.to_string(), "订阅单", "subscribed")
                    .unwrap();
            }
        }
        let playlists_with_tracks = |db: &mut LibraryDb| {
            db.list_playlists()
                .unwrap()
                .into_iter()
                .filter(|p| p.track_count > 0)
                .count()
        };

        // 首轮：预算内 24 个补到曲目；本遍未走完 → 续跑时刻在短延时之后。
        worker.reconcile().await;
        assert_eq!(playlists_with_tracks(&mut library.lock().unwrap()), 24);
        let until = worker
            .tracks_resume_at
            .expect("本遍未走完应排续跑")
            .saturating_duration_since(tokio::time::Instant::now());
        assert!(
            until > TRACKS_RESUME_DELAY / 2 && until <= TRACKS_RESUME_DELAY,
            "续跑应排在约 5 秒后，实际 {until:?}"
        );

        // 续跑片：走完余下 6 个 → 本遍结束，续跑撤销；有变更 → 推库变更。
        assert!(worker.resume_playlist_tracks().await);
        assert!(worker.tracks_resume_at.is_none(), "本遍走完不再续跑");
        assert_eq!(playlists_with_tracks(&mut library.lock().unwrap()), 30);
        assert!(changed_rx.has_changed().unwrap(), "曲目缓存变更应推库变更");

        // 再触发一轮：从头分片，续跑片同快照零变更 → 不推库变更。
        worker.reconcile().await;
        assert!(worker.tracks_resume_at.is_some(), "新一轮同样未走完");
        let generation = *changed_rx.borrow_and_update();
        assert!(
            !worker.resume_playlist_tracks().await,
            "同快照零变更不推库变更"
        );
        assert!(worker.tracks_resume_at.is_none());
        assert_eq!(*changed_rx.borrow(), generation);
    }
}
