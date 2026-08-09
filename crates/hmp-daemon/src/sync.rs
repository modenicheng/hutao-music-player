//! QQ 乐观同步 worker（本地先提交、远端趋同；spec §3.2）。
//!
//! 消费三张 durable outbox：`relations`（收藏/订阅，含操作合并后的最终意图）、
//! `playlists`（owned 歌单创建/删除）、`playlist_ops`（owned 歌单曲目增删）。
//! 无凭证时整体跳过（离线意图合法留存）；失败指数退避重试
//! （`min(2^retry_count * 10s, 10min)`，由行内 retry_count 驱动）。
//!
//! 触发：写命令经 [`SyncHandle::trigger`] 通知；30s 定时兜底扫描。

use std::sync::{Arc, Mutex};

use hmp_qqmusic_api::{
    QqMusicClient, album::AlbumApi, credential::Credential, song::SongApi, songlist::SonglistApi,
};
use hmp_storage::{LibraryDb, RelationRow};
use tokio::sync::mpsc;

/// worker 消息。
#[derive(Clone, Copy)]
enum SyncMsg {
    /// 消费 outbox（写命令落地后触发）。
    Sync,
    /// 从 QQ 拉用户库快照 reconcile（`hmp library sync`）。
    Reconcile,
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
}

/// 单次重试间隔：`min(2^retry * 10s, 10min)`。
fn retry_delay(retry_count: i64) -> std::time::Duration {
    let secs = 10i64.saturating_mul(1i64 << retry_count.min(6));
    std::time::Duration::from_secs(secs.min(600) as u64)
}

impl SyncWorker {
    /// 启动 worker，返回触发句柄。
    pub fn spawn(
        library: Arc<Mutex<LibraryDb>>,
        client: QqMusicClient,
        store: Box<dyn hmp_storage::credential::CredentialStore>,
    ) -> SyncHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut worker = Self {
            library,
            client,
            store,
            notify: rx,
        };
        tokio::spawn(async move { worker.run().await });
        SyncHandle { notify: tx }
    }

    async fn run(&mut self) {
        loop {
            let msg = tokio::select! {
                msg = self.notify.recv() => msg,
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => Some(SyncMsg::Sync),
            };
            match msg {
                Some(SyncMsg::Sync) => self.sync_once().await,
                Some(SyncMsg::Reconcile) => self.reconcile().await,
                None => return, // 通道关闭（daemon 退出）
            }
        }
    }

    /// QQ 用户库快照 reconcile（Phase 4；未登录 → 跳过）。
    async fn reconcile(&mut self) {
        let Some(credential) = self.load_credential() else {
            return;
        };
        crate::reconcile::reconcile_user_library(&self.client, &credential, &self.library).await;
    }

    /// 一轮同步：relations → playlists（owned）→ playlist_ops。
    async fn sync_once(&mut self) {
        let Some(credential) = self.load_credential() else {
            return; // 无凭证：离线意图留 outbox（合法状态）
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut relations = {
            let Ok(mut lib) = self.library.lock() else {
                return;
            };
            lib.relations_pending().unwrap_or_default()
        };
        // 指数退避：error 行在上次尝试后的退避窗口内不重试。
        relations.retain(|r| {
            r.sync_state == "pending"
                || now - r.updated_at >= retry_delay(r.retry_count).as_secs() as i64
        });
        for row in relations {
            self.sync_relation(&row, &credential).await;
        }
        let playlists = {
            let Ok(mut lib) = self.library.lock() else {
                return;
            };
            lib.playlists_pending().unwrap_or_default()
        };
        for row in playlists {
            self.sync_playlist(&row, &credential).await;
        }
        let ops = {
            let Ok(mut lib) = self.library.lock() else {
                return;
            };
            lib.playlist_ops_pending().unwrap_or_default()
        };
        for op in ops {
            self.sync_playlist_op(&op, &credential).await;
        }
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
                    None => return, // 未知 numeric id：留 pending，等待播放/解析补全
                }
            }
            ("playlist", "subscribed") => {
                let disstid: i64 = match key.parse() {
                    Ok(v) => v,
                    Err(_) => {
                        return self.fail_relation(row, "歌单 id 非数字");
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
                        return self.fail_relation(row, "专辑 id 非数字");
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
            Ok(resp) => Err(format!("QQ 创建歌单失败: ret_code={}", resp.ret_code)),
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
                    return; // numeric id 未知：等待补全后 30s 兜底重扫
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
                    Ok(resp) => Err(format!("QQ 删除歌单失败: ret_code={}", resp.ret_code)),
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
}
