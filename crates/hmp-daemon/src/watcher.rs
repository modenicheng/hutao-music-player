//! 本地目录监听（里程碑 E2）：scan_roots 变化自动入库。
//!
//! 事件驱动单文件处理：不推进 generation、不批量标 missing（缺失标记
//! 由手动全量扫描负责——复制大目录中途不会被误标）。删除事件 → 单文件
//! missing 标记；移动 = notify 的 Create(新路径)+Remove(旧路径) → 指纹
//! 命中自动复用行、旧路径 mark_missing 查无行无操作。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hmp_storage::{LibraryDb, read_meta};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// 监听器句柄（drop = 终止批处理任务；任务持有的 watcher 随之释放，监听停止）。
pub struct LocalWatcher {
    _watcher: Arc<Mutex<RecommendedWatcher>>,
    _task: tokio::task::JoinHandle<()>,
}

impl Drop for LocalWatcher {
    fn drop(&mut self) {
        // 终止批处理任务：任务持有 watcher 副本，abort 后监听随 runtime 释放停止。
        self._task.abort();
    }
}

/// 事件批处理队列（notify handler 线程只收集；后台任务每 1s 消费）。
#[derive(Default)]
struct WatchQueue {
    pending: Mutex<HashSet<PathBuf>>,
}

impl WatchQueue {
    fn push(&self, p: PathBuf) {
        self.pending.lock().unwrap().insert(p);
    }
    fn drain(&self) -> Vec<PathBuf> {
        std::mem::take(&mut *self.pending.lock().unwrap())
            .into_iter()
            .collect()
    }
}

impl LocalWatcher {
    /// 启动监听。无 scan_roots / 全部 root 监听失败 → None（warn 不阻断 daemon）。
    pub fn spawn(library: Arc<Mutex<LibraryDb>>) -> Option<Self> {
        let roots: Vec<String> = {
            let mut lib = library.lock().ok()?;
            match lib.scan_roots() {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(%e, "读取 scan_roots 失败");
                    return None;
                }
            }
        };
        if roots.is_empty() {
            return None;
        }
        let queue = Arc::new(WatchQueue::default());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>(); // 唤醒批处理任务
        // notify handler：仅收集路径（轻量，不阻塞 notify 线程）。
        let handler_queue = Arc::clone(&queue);
        let handler_tx = tx.clone();
        let mut watcher =
            match notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
                if let Ok(ev) = res {
                    match ev.kind {
                        EventKind::Create(_) | EventKind::Modify(_) => {
                            for p in ev.paths {
                                if hmp_storage::is_audio_ext(&p) {
                                    handler_queue.push(p);
                                }
                            }
                        }
                        EventKind::Remove(_) => {
                            for p in ev.paths {
                                handler_queue.push(p); // 删除也进队（音频过滤在消费侧）
                            }
                        }
                        _ => {}
                    }
                    let _ = handler_tx.send(());
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!(%e, "notify watcher 创建失败");
                    return None;
                }
            };
        let mut watched: HashSet<PathBuf> = HashSet::new();
        for root in &roots {
            let p = PathBuf::from(root);
            match watcher.watch(&p, RecursiveMode::Recursive) {
                Ok(()) => {
                    watched.insert(p);
                }
                Err(e) => tracing::warn!(%e, root = %p.display(), "监听扫描根失败（外接盘离线？）"),
            }
        }
        if watched.is_empty() {
            return None;
        }
        // 后台批处理任务：每 1s 或收到唤醒即消费队列。
        // watcher 经 Arc<Mutex> 共享：struct 保活一份，task 持一份（注册新 root）。
        let watcher = Arc::new(Mutex::new(watcher));
        let task_library = Arc::clone(&library);
        let task_queue = Arc::clone(&queue);
        let task_watcher = Arc::clone(&watcher);
        let mut watched_task = watched.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = rx.recv() => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
                let paths = task_queue.drain();
                // 按文件短锁：拷贝大目录时每文件 1MB 指纹读不长时间阻塞 engine/sync。
                for p in paths {
                    let mut lib = match task_library.lock() {
                        Ok(l) => l,
                        Err(_) => continue,
                    };
                    Self::handle_event(&mut lib, &p);
                    drop(lib);
                }
                // 每轮无条件刷新：空闲时新增 scan_roots（CLI 新扫描目录/外接盘恢复）
                // 也能在 1s 内注册监听。
                Self::refresh_roots(&task_library, &task_watcher, &mut watched_task).await;
            }
        });
        Some(Self {
            _watcher: watcher,
            _task: task,
        })
    }

    /// 消费一个事件路径（音频 → upsert/指纹复用；删除 → missing 标记）。
    fn handle_event(lib: &mut LibraryDb, p: &PathBuf) {
        if p.exists() {
            if !hmp_storage::is_audio_ext(p) {
                return;
            }
            // 找到所属 root（前缀匹配）拿 root_id/当前 gen（不推进）。
            let Ok(Some((root_id, generation))) = lib.scan_root_for(p) else {
                return;
            };
            let md = match std::fs::metadata(p) {
                Ok(m) => m,
                Err(_) => return,
            };
            let size = md.len();
            // 指纹读失败：跳过该事件（空指纹会污染 find_by_fingerprint 候选）。
            let Ok(fp) = hmp_storage::scan::file_fingerprint(p, size) else {
                return;
            };
            let local_meta = read_meta(p);
            let cover_uri = match &local_meta {
                Some(m) => m
                    .cover
                    .as_deref()
                    .and_then(|c| hmp_storage::scan::persist_cover(c).ok()),
                None => None,
            };
            match lib.record_scan_file(root_id, generation, p, local_meta.as_ref(), &fp) {
                Ok(_) => {
                    if let Some(uri) = &cover_uri {
                        let _ = lib.set_track_cover(&format!("local:{}", p.display()), uri);
                    }
                }
                Err(e) => tracing::warn!(%e, path = %p.display(), "watcher 入库失败"),
            }
        } else if hmp_storage::is_audio_ext(p) {
            match lib.mark_missing_by_path(p) {
                Ok(n) => {
                    if n > 0 {
                        tracing::debug!(path = %p.display(), "文件缺失标记");
                    }
                }
                Err(e) => tracing::warn!(%e, path = %p.display(), "watcher 删除标记失败"),
            }
        }
    }

    /// 周期刷新 scan_roots：新增 root 增量注册监听。
    async fn refresh_roots(
        library: &Arc<Mutex<LibraryDb>>,
        watcher: &Arc<Mutex<RecommendedWatcher>>,
        watched: &mut HashSet<PathBuf>,
    ) {
        let roots: Vec<String> = library
            .lock()
            .ok()
            .and_then(|mut l| l.scan_roots().ok())
            .unwrap_or_default();
        for root in roots {
            let p = PathBuf::from(root);
            if watched.contains(&p) {
                continue;
            }
            let mut w = watcher.lock().unwrap();
            match w.watch(&p, RecursiveMode::Recursive) {
                Ok(()) => {
                    watched.insert(p);
                }
                Err(e) => tracing::warn!(%e, root = %p.display(), "监听新扫描根失败"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use hmp_storage::LibraryDb;

    fn temp_env() -> (tempfile::TempDir, tempfile::TempDir) {
        // (库目录, 音乐目录)：库用文件库（scan_roots 持久），音乐目录空。
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap())
    }

    #[tokio::test]
    async fn watcher_ingests_new_and_deleted_files() {
        let (lib_dir, music) = temp_env();
        let db_path = lib_dir.path().join("library.sqlite3");
        let mut db = LibraryDb::open(&db_path).unwrap();
        // 注册扫描根（模拟 CLI scan 注册）。
        db.begin_scan(music.path()).unwrap();
        drop(db);
        let library = Arc::new(Mutex::new(LibraryDb::open(&db_path).unwrap()));
        let Some(watcher) = LocalWatcher::spawn(library.clone()) else {
            eprintln!("跳过：环境不支持目录监听（无 inotify？）");
            return;
        };
        // 写入音频文件 → 事件 → 自动入库（轮询等待，超时 5s）。
        let f = music.path().join("new-song.mp3");
        std::fs::write(&f, b"abc").unwrap();
        let key = format!("local:{}", f.canonicalize().unwrap().display());
        let mut found = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let mut lib = library.lock().unwrap();
            if lib.track_id("local", &key).unwrap().is_some() {
                found = true;
                break;
            }
        }
        assert!(found, "新增文件应自动入库: {key}");
        // 删除 → missing 标记（独立只读连接查询；轮询等待）。
        std::fs::remove_file(&f).unwrap();
        let mut missing = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let check = rusqlite::Connection::open(&db_path).unwrap();
            let miss: i64 = check
                .query_row(
                    "SELECT missing FROM local_files WHERE path=?1",
                    [f.display().to_string()],
                    |r| r.get(0),
                )
                .unwrap_or(-1);
            if miss == 1 {
                missing = true;
                break;
            }
        }
        assert!(missing, "删除文件应标 missing");
        drop(watcher);
    }

    #[tokio::test]
    async fn watcher_ignores_non_audio_and_no_roots() {
        // 无 scan_roots → spawn 返回 None。
        let (lib_dir, _) = temp_env();
        let db_path = lib_dir.path().join("library.sqlite3");
        let library = Arc::new(Mutex::new(LibraryDb::open(&db_path).unwrap()));
        assert!(
            LocalWatcher::spawn(library.clone()).is_none(),
            "无 root 不启动"
        );
        // 非音频文件事件不入库。
        let music = tempfile::tempdir().unwrap();
        {
            let mut db = library.lock().unwrap();
            db.begin_scan(music.path()).unwrap();
        }
        let Some(watcher) = LocalWatcher::spawn(library.clone()) else {
            eprintln!("跳过：环境不支持目录监听");
            return;
        };
        let f = music.path().join("note.txt");
        std::fs::write(&f, b"hello").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let n: i64 = {
            let check = rusqlite::Connection::open(&db_path).unwrap();
            check
                .query_row(
                    "SELECT COUNT(*) FROM tracks WHERE source='local'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(n, 0, "非音频不应入库");
        drop(watcher);
    }
}
