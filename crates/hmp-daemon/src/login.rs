//! 扫码登录服务（桌面端 `LoginQr*` IPC 的 daemon 侧实现；AUDIT §8.6 延伸——
//! 凭证操作统一在 daemon，客户端不直连 QQ）。
//!
//! 流程与 CLI `hmp login` 同契约：取二维码 → 轮询 → 超时自动刷新（总墙钟
//! 10 分钟）→ 成功落凭证库。差异点：二维码 PNG 落盘 `data_dir/`（UI 禁 HTTP，
//! 直读本机文件）；轮询无状态化（客户端 `LoginQrPoll` 短连接驱动）。
//!
//! 并发模型：会话状态在 `std::sync::Mutex<Option<Session>>`，跨 `.await`
//! 一律「短锁快照 → 出网 → 带同值判定回写」——单连接轮询/多客户端并发轮询、
//! 轮询与 Start/Cancel 竞争都只影响过期快照，不会串会话。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hmp_core::LoginQrSession;
use hmp_core::ipc::LoginQrState;
use hmp_qqmusic_api::{LoginApi, QR, QRCodeLoginEvents, QRLoginType, QqMusicClient};
use hmp_storage::credential::Store;

/// 总墙钟上限：二维码无限过期也不死循环（CLI `hmp login` 同值）。
const OVERALL_LIMIT: Duration = Duration::from_secs(600);

/// 进行中的登录会话。
struct Session {
    qr: QR,
    qr_path: PathBuf,
    deadline: Instant,
}

/// 出网前的会话快照（同值判定 = qr_path + deadline；避免引入代次计数）。
struct SessionSnapshot {
    qr: QR,
    qr_path: PathBuf,
    deadline: Instant,
}

impl Session {
    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            qr: self.qr.clone(),
            qr_path: self.qr_path.clone(),
            deadline: self.deadline,
        }
    }
}

/// 扫码登录服务（daemon 持有；server 经 `EngineHandle.login` 调用）。
pub struct LoginService {
    client: QqMusicClient,
    store: Store,
    session: Arc<Mutex<Option<Session>>>,
    /// 二维码文件名代次（同会话刷新换新文件，UI 按 path 变化重载图）。
    qr_counter: AtomicU64,
}

impl LoginService {
    /// 新建（`store` 由 `store_from_env()` 构造）。
    pub fn new(store: Store) -> Self {
        Self {
            client: QqMusicClient::new(),
            store,
            session: Arc::new(Mutex::new(None)),
            qr_counter: AtomicU64::new(1),
        }
    }

    /// 发起登录：生成二维码并落盘，替换进行中的旧会话（如有，顺带清图）。
    pub async fn start(&self) -> Result<LoginQrSession, String> {
        let qr = LoginApi::new(&self.client)
            .get_qrcode(QRLoginType::Qq)
            .await
            .map_err(|e| e.to_string())?;
        let path = self.write_qr_png(&qr)?;
        let old = self
            .session
            .lock()
            .expect("login session")
            .replace(Session {
                qr,
                qr_path: path.clone(),
                deadline: Instant::now() + OVERALL_LIMIT,
            });
        if let Some(old) = old {
            let _ = std::fs::remove_file(&old.qr_path);
        }
        Ok(LoginQrSession {
            qr_path: path.to_string_lossy().into_owned(),
        })
    }

    /// 轮询进度。无会话 / 总墙钟耗尽 / 出错 → `LoginQrState::STATUS_IDLE` 携带原因（非协议错误）。
    pub async fn poll(&self) -> LoginQrState {
        let Some(snapshot) = self.session_snapshot() else {
            return self.idle("没有进行中的扫码登录".into());
        };
        if Instant::now() >= snapshot.deadline {
            self.clear_if_same(&snapshot);
            return self.idle("登录超时（10 分钟上限），请重新发起扫码登录".into());
        }
        let result = LoginApi::new(&self.client).check_qrcode(&snapshot.qr).await;
        match result {
            Ok(result) => match result.event {
                QRCodeLoginEvents::Conf => LoginQrState {
                    status: LoginQrState::STATUS_SCANNED,
                    qr_path: snapshot.qr_path.to_string_lossy().into_owned(),
                    message: String::new(),
                },
                QRCodeLoginEvents::Scan => LoginQrState {
                    status: LoginQrState::STATUS_WAITING,
                    qr_path: snapshot.qr_path.to_string_lossy().into_owned(),
                    message: String::new(),
                },
                QRCodeLoginEvents::Done => {
                    let Some(credential) = result.credential else {
                        self.clear_if_same(&snapshot);
                        return self.idle("登录完成但响应缺少凭证".into());
                    };
                    match self.store.save(&credential) {
                        Ok(()) => {
                            self.clear_if_same(&snapshot);
                            LoginQrState {
                                status: LoginQrState::STATUS_DONE,
                                qr_path: String::new(),
                                message: String::new(),
                            }
                        }
                        // 保存失败保留会话：keyring 瞬时故障可重试轮询。
                        Err(e) => self.idle(format!("凭证保存失败: {e}")),
                    }
                }
                QRCodeLoginEvents::Refuse => {
                    self.clear_if_same(&snapshot);
                    self.idle("已在手机上拒绝登录".into())
                }
                // 二维码过期（非总墙钟）：自动刷新重出图，会话延续（CLI 同语义）。
                QRCodeLoginEvents::Timeout => match self.refresh(&snapshot).await {
                    Ok(path) => LoginQrState {
                        status: LoginQrState::STATUS_WAITING,
                        qr_path: path.to_string_lossy().into_owned(),
                        message: "二维码已过期，已自动刷新".into(),
                    },
                    Err(e) => {
                        self.clear_if_same(&snapshot);
                        self.idle(format!("二维码刷新失败: {e}"))
                    }
                },
            },
            // 非超时类错误（网络/协议）：终态化，客户端提示后可重试。
            Err(e) => {
                self.clear_if_same(&snapshot);
                self.idle(e.to_string())
            }
        }
    }

    /// 取消会话（幂等；无会话也是 Ok）。
    pub fn cancel(&self) {
        let session = self.session.lock().expect("login session").take();
        if let Some(session) = session {
            let _ = std::fs::remove_file(&session.qr_path);
        }
    }

    /// 退出登录：远端登出尽力而为（网络失败不阻断），本地凭证必删。
    /// 未登录幂等 Ok。
    pub async fn logout(&self) -> Result<(), String> {
        let credential = self
            .store
            .load()
            .map_err(|e| format!("failed to read credentials: {e}"))?
            .filter(|c| c.is_logged_in());
        if let Some(credential) = credential {
            if let Err(e) = LoginApi::new(&self.client).logout(&credential).await {
                // 远端登出失败不影响本地删除（凭证已失效亦可接受）。
                tracing::warn!(%e, "remote logout failed; removing local credential anyway");
            }
        }
        self.store
            .delete()
            .map_err(|e| format!("failed to delete credentials: {e}"))
    }

    /// 会话快照（短锁克隆；`QR` 携带的数据均为 owned）。
    fn session_snapshot(&self) -> Option<SessionSnapshot> {
        let guard = self.session.lock().expect("login session");
        guard.as_ref().map(Session::snapshot)
    }

    /// 终态化：仅当会话未被并发 Start 替换时清除并清图。
    fn clear_if_same(&self, snapshot: &SessionSnapshot) {
        let mut guard = self.session.lock().expect("login session");
        let same = guard
            .as_ref()
            .is_some_and(|s| s.qr_path == snapshot.qr_path && s.deadline == snapshot.deadline);
        if same {
            if let Some(session) = guard.take() {
                let _ = std::fs::remove_file(&session.qr_path);
            }
        }
    }

    /// 刷新二维码（超时续会话）：新图新文件，旧图即删。
    async fn refresh(&self, snapshot: &SessionSnapshot) -> Result<PathBuf, String> {
        let qr = LoginApi::new(&self.client)
            .get_qrcode(QRLoginType::Qq)
            .await
            .map_err(|e| e.to_string())?;
        let path = self.write_qr_png(&qr)?;
        let mut guard = self.session.lock().expect("login session");
        let same = guard
            .as_ref()
            .is_some_and(|s| s.qr_path == snapshot.qr_path && s.deadline == snapshot.deadline);
        if same {
            if let Some(session) = guard.as_mut() {
                let old = std::mem::replace(&mut session.qr_path, path.clone());
                session.qr = qr;
                let _ = std::fs::remove_file(&old);
            }
        }
        Ok(path)
    }

    /// 写二维码 PNG 到 `data_dir/login-qr-{n}.png`（目录不存在则创建）。
    fn write_qr_png(&self, qr: &QR) -> Result<PathBuf, String> {
        let n = self.qr_counter.fetch_add(1, Ordering::Relaxed);
        let path = hmp_storage::data_dir().join(format!("login-qr-{n}.png"));
        Self::write_png(&path, qr)
    }

    fn write_png(path: &Path, qr: &QR) -> Result<PathBuf, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create login qr dir: {e}"))?;
        }
        std::fs::write(path, &qr.data).map_err(|e| format!("write login qr png: {e}"))?;
        Ok(path.to_path_buf())
    }

    fn idle(&self, message: String) -> LoginQrState {
        LoginQrState {
            status: LoginQrState::STATUS_IDLE,
            qr_path: String::new(),
            message,
        }
    }
}

/// daemon 启动时清理上次会话遗留的二维码图（登录中断/崩溃残留）。
pub fn cleanup_stale_qr_files() {
    cleanup_stale_qr_files_in(&hmp_storage::data_dir());
}

fn cleanup_stale_qr_files_in(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("login-qr-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmp_qqmusic_api::credential::Credential;
    use hmp_storage::credential::FileStore;

    fn service_at(dir: &std::path::Path) -> LoginService {
        LoginService::new(Box::new(FileStore::at(dir.join("cred.json"))))
    }

    /// 未登录（空 store）登出：幂等 Ok，不触碰网络。
    #[tokio::test]
    async fn logout_without_credential_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        service_at(dir.path()).logout().await.unwrap();
    }

    /// 无会话 poll：LoginQrState::STATUS_IDLE + 提示文案（非错误）。
    #[tokio::test]
    async fn poll_without_session_is_idle() {
        let dir = tempfile::tempdir().unwrap();
        let state = service_at(dir.path()).poll().await;
        assert_eq!(state.status, LoginQrState::STATUS_IDLE);
        assert!(!state.message.is_empty());
    }

    /// cancel 无会话幂等；会话持有的二维码图随 cancel 删除。
    #[tokio::test]
    async fn cancel_removes_qr_file() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service_at(dir.path());
        svc.cancel();
        let (qr, path) = fake_session_png(&svc);
        assert!(path.exists());
        svc.session.lock().unwrap().replace(Session {
            qr,
            qr_path: path.clone(),
            deadline: Instant::now() + OVERALL_LIMIT,
        });
        svc.cancel();
        assert!(!path.exists(), "cancel 应删除二维码图");
        assert!(svc.session.lock().unwrap().is_none());
    }

    /// 写二维码 PNG 自动创建父目录；文件内容原样落盘。
    #[test]
    fn write_png_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("login-qr-1.png");
        let qr = fake_qr();
        LoginService::write_png(&path, &qr).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), qr.data);
    }

    /// cleanup 只清 login-qr- 前缀，不动同目录其他文件。
    #[test]
    fn cleanup_removes_only_login_qr_files() {
        let dir = tempfile::tempdir().unwrap();
        let keep = dir.path().join("library.sqlite3");
        let stale = dir.path().join("login-qr-7.png");
        std::fs::write(&keep, b"x").unwrap();
        std::fs::write(&stale, b"x").unwrap();
        cleanup_stale_qr_files_in(dir.path());
        assert!(keep.exists());
        assert!(!stale.exists());
    }

    fn fake_qr() -> QR {
        QR {
            data: b"png-bytes".to_vec(),
            qr_type: QRLoginType::Qq,
            mimetype: "image/png".into(),
            identifier: "qrsig-test".into(),
        }
    }

    /// 借 write_png 落一张真图并构造会话数据（不依赖网络）。
    fn fake_session_png(svc: &LoginService) -> (QR, PathBuf) {
        let qr = fake_qr();
        let path = svc.write_qr_png(&qr).unwrap();
        (qr, path)
    }

    /// 凭证构造样例（供后续含 store 写入的用例扩展）。
    #[allow(dead_code)]
    fn sample_credential() -> Credential {
        Credential {
            uin: "10001".into(),
            music_id: "10001".into(),
            music_key: "k".into(),
            refresh_key: None,
            login_type: Default::default(),
            raw_cookie: String::new(),
            openid: String::new(),
            refresh_token: String::new(),
            access_token: String::new(),
            str_musicid: "10001".into(),
            ..Default::default()
        }
    }
}
