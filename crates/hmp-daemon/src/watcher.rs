//! 本地目录监听（里程碑 E2）：scan_roots 变化自动入库。
//!
//! 事件驱动单文件处理：不推进 generation、不批量标 missing（缺失标记
//! 由手动全量扫描负责——复制大目录中途不会被误标）。删除事件 → 单文件
//! missing 标记；移动 = notify 的 Create(新路径)+Remove(旧路径) → 指纹
//! 命中自动复用行、旧路径 mark_missing 查无行无操作。
//!
//! 目录封面（`cover.jpg` 等，候选集合见 `hmp_storage::local`）也走同一队列：
//! 新增/替换/删除/改名后，重读同目录已入库本地曲目的元数据并只回写实际
//! 变化的 `cover_uri`——扫描后才放进专辑目录的封面无需再等全量扫描。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
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
    /// `library_changed`：批处理落库后 bump（server 推 `LibraryChanged`，AUDIT §8.9）。
    pub fn spawn(
        library: Arc<Mutex<LibraryDb>>,
        library_changed: tokio::sync::watch::Sender<u64>,
    ) -> Option<Self> {
        let roots: Vec<String> = {
            let mut lib = library.lock().ok()?;
            match lib.scan_roots() {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(%e, "failed to read scan roots");
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
                        // Remove 也按文件名过滤：音频 → missing，封面 → 目录封面
                        // 刷新；路径在文件消失后仍可判断，不顺带的路径不进队。
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                            for p in ev.paths {
                                if Self::is_event_path(&p) {
                                    handler_queue.push(p);
                                }
                            }
                        }
                        _ => {}
                    }
                    let _ = handler_tx.send(());
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!(%e, "failed to create notify watcher");
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
                Err(e) => {
                    tracing::warn!(%e, root = %p.display(), "failed to watch scan root (drive offline?)")
                }
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
        let task_notify = library_changed;
        let mut watched_task = watched.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = rx.recv() => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
                let paths = task_queue.drain();
                // 按文件短锁：拷贝大目录时每文件 1MB 指纹读不长时间阻塞 engine/sync。
                let mut changed = false;
                // 同一事件批里同一目录可能有多条封面路径（改名 = Remove+Create、
                // 一次拖入多张图）：归并后每目录只重读一遍。
                let mut cover_dirs: HashSet<PathBuf> = HashSet::new();
                for p in paths {
                    // 封面事件绝不进音频入库分支：候选扩展名只有图片，这里先
                    // 分流也防止未来候选集合与音频扩展名重叠。
                    if hmp_storage::is_folder_cover_path(&p) {
                        if let Some(dir) = p.parent() {
                            cover_dirs.insert(dir.to_path_buf());
                        }
                        continue;
                    }
                    if !hmp_storage::is_audio_ext(&p) {
                        continue;
                    }
                    let mut lib = match task_library.lock() {
                        Ok(l) => l,
                        Err(_) => continue,
                    };
                    changed |= Self::handle_audio_event(&mut lib, &p);
                    drop(lib);
                }
                // 封面重读（读标签、封面落盘）在库锁外；短锁只读写行。
                for dir in cover_dirs {
                    changed |= Self::refresh_folder_cover(&task_library, &dir);
                }
                if changed {
                    task_notify.send_modify(|g| *g += 1);
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

    /// 路径是否值得进队：音频或目录封面候选。纯文件名判断——删除事件时
    /// 文件已不存在，仍能分清「该标 missing」与「该刷新目录封面」。
    fn is_event_path(p: &Path) -> bool {
        hmp_storage::is_audio_ext(p) || hmp_storage::is_folder_cover_path(p)
    }

    /// 消费一个音频事件路径（新增/更新 → upsert/指纹复用；删除 → missing 标记）。
    /// 返回库内容是否可能变化（决定是否 bump `LibraryChanged`）。
    fn handle_audio_event(lib: &mut LibraryDb, p: &Path) -> bool {
        if p.exists() {
            // 找到所属 root（前缀匹配）拿 root_id/当前 gen（不推进）。
            let Ok(Some((root_id, generation))) = lib.scan_root_for(p) else {
                return false;
            };
            let md = match std::fs::metadata(p) {
                Ok(m) => m,
                Err(_) => return false,
            };
            let size = md.len();
            // 指纹读失败：跳过该事件（空指纹会污染 find_by_fingerprint 候选）。
            let Ok(fp) = hmp_storage::scan::file_fingerprint(p, size) else {
                return false;
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
                    true
                }
                Err(e) => {
                    tracing::warn!(%e, path = %p.display(), "watcher ingestion failed");
                    false
                }
            }
        } else {
            match lib.mark_missing_by_path(p) {
                Ok(n) => {
                    if n > 0 {
                        tracing::debug!(path = %p.display(), "marked file missing");
                    }
                    n > 0
                }
                Err(e) => {
                    tracing::warn!(%e, path = %p.display(), "watcher missing-mark failed");
                    false
                }
            }
        }
    }

    /// 目录封面变化：重读同目录已入库本地曲目的元数据，回写 `cover_uri`。
    ///
    /// 封面来源语义完全复用 `read_meta`（内嵌图优先）：新增/替换目录封面时
    /// 只有无内嵌图的曲目改用新封面；有内嵌图的不被覆盖；删掉目录封面后
    /// 自动回退到内嵌图，两者都没有才清空。
    ///
    /// 只回写实际变化的行：同目录无内嵌图的曲目共享同一个目录封面 URI，
    /// 第一行写库后其余行零写入（`update_track_cover` 的 SQL 也保证
    /// 值未变不产生写）。重 I/O（读标签、封面落盘）全部在库锁外：
    /// 短锁读行 → 锁外重读 → 短锁回写。返回是否有行变化。
    fn refresh_folder_cover(library: &Arc<Mutex<LibraryDb>>, dir: &Path) -> bool {
        let rows = {
            let mut lib = match library.lock() {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(%e, "folder cover: library lock poisoned");
                    return false;
                }
            };
            match lib.local_tracks_in_dir(dir) {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::warn!(%e, dir = %dir.display(), "folder cover: query failed");
                    return false;
                }
            }
        };
        if rows.is_empty() {
            return false; // 目录里没有已入库曲目：封面事件不入库
        }
        let mut updates: Vec<(String, Option<String>)> = Vec::new();
        for row in &rows {
            let path = Path::new(&row.path);
            let Some(meta) = read_meta(path) else {
                // 文件读不了（扫描间隙被删/损坏）：保留现有封面，删除与
                // missing 语义交给音频事件，不能据此清空。
                continue;
            };
            let next = match meta.cover.as_deref() {
                Some(bytes) => match hmp_storage::scan::persist_cover(bytes) {
                    Ok(uri) => Some(uri),
                    Err(e) => {
                        // 落盘失败保留旧值：清空会让封面在 UI 上凭空消失。
                        tracing::warn!(%e, path = %path.display(), "folder cover: persist failed");
                        continue;
                    }
                },
                None => None,
            };
            if next != row.cover_uri {
                updates.push((row.source_key.clone(), next));
            }
        }
        if updates.is_empty() {
            return false;
        }
        let mut lib = match library.lock() {
            Ok(l) => l,
            Err(_) => return false,
        };
        let mut changed = false;
        for (source_key, cover_uri) in updates {
            match lib.update_track_cover(&source_key, cover_uri.as_deref()) {
                Ok(n) => changed |= n > 0,
                Err(e) => {
                    tracing::warn!(%e, source_key = %source_key, "folder cover: update failed")
                }
            }
        }
        changed
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
                Err(e) => tracing::warn!(%e, root = %p.display(), "failed to watch new scan root"),
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
        let (notify, _notify_rx) = tokio::sync::watch::channel(0u64);
        let Some(watcher) = LocalWatcher::spawn(library.clone(), notify) else {
            eprintln!("跳过：环境不支持目录监听（无 inotify？）");
            return;
        };
        // 写入音频文件 → 事件 → 自动入库（轮询等待，超时 5s）。
        // 路径统一从 canonical 根构造：Windows 上 TEMP 环境变量拼写与盘上
        // 真实大小写可能不一；文件删除后无法 canonicalize，校验查询必须与
        // 事件拼写（监听根 = DB canonical 拼写）同源。
        let root = hmp_storage::canonical_display_path(music.path());
        let f = root.join("new-song.mp3");
        std::fs::write(&f, b"abc").unwrap();
        let key = format!("local:{}", f.display());
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
            LocalWatcher::spawn(library.clone(), tokio::sync::watch::channel(0u64).0).is_none(),
            "无 root 不启动"
        );
        // 非音频文件事件不入库。
        let music = tempfile::tempdir().unwrap();
        {
            let mut db = library.lock().unwrap();
            db.begin_scan(music.path()).unwrap();
        }
        let (notify, _notify_rx) = tokio::sync::watch::channel(0u64);
        let Some(watcher) = LocalWatcher::spawn(library.clone(), notify) else {
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

    // —— 目录封面（`read_meta` 的 folder_cover 兜底）——

    /// 三张内容不同的 1×1 PNG：内嵌图 / 目录封面 / 大小写变体封面，靠字节
    /// 区分来源（URI 由内容哈希决定，字节不同即 URI 不同）。
    const EMBEDDED_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    const FOLDER_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60,
        0xF8, 0xCF, 0xF0, 0x1F, 0x00, 0x04, 0x01, 0x01, 0xFF, 0x71, 0xEB, 0x47, 0xE5, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    const FRONT_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60,
        0x60, 0xF8, 0xFF, 0x1F, 0x00, 0x03, 0x02, 0x01, 0xFF, 0xE6, 0x77, 0x0B, 0xAE, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// 封面落盘目录隔离：`persist_cover` 写 `$XDG_DATA_HOME/hmp/covers`。
    /// daemon 测试套件里没有其它读者（登录测试用注入目录、content 测试只碰
    /// `XDG_CACHE_HOME`）；改 env 的两个封面用例由本锁互斥串行。
    static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct IsolatedDataDir {
        _dir: tempfile::TempDir,
    }

    impl IsolatedDataDir {
        /// 调用方必须持有 [`TEST_ENV_LOCK`]（guard 要活到测试结束）。
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            // SAFETY: 调用方持有 TEST_ENV_LOCK，改 env 的测试互斥串行。
            unsafe {
                std::env::set_var("XDG_DATA_HOME", dir.path());
            }
            Self { _dir: dir }
        }
    }

    impl Drop for IsolatedDataDir {
        fn drop(&mut self) {
            // SAFETY: 同上（panic 路径也恢复，避免污染后续测试）。
            unsafe {
                std::env::remove_var("XDG_DATA_HOME");
            }
        }
    }

    /// 最小合法 WAV（8kHz 单声道 16bit PCM；与 hmp-storage `local.rs` 测试
    /// 同构），可带 ID3v2.4 APIC 内嵌 PNG（CoverFront）。`seed` 填充采样数据：
    /// 内容不同的文件才有不同指纹——同内容同 mtime 会被 `record_scan_file`
    /// 判成「移动」而复用同一行。daemon 不依赖 lofty/image 的写侧 API，
    /// 测试手造容器。
    fn minimal_wav(seed: u8, embedded_png: Option<&[u8]>) -> Vec<u8> {
        fn syncsafe(n: usize) -> [u8; 4] {
            [21, 14, 7, 0].map(|shift| ((n >> shift) & 0x7f) as u8)
        }
        fn push_chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], body: &[u8]) {
            out.extend_from_slice(fourcc);
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(body);
            if body.len() % 2 == 1 {
                out.push(0); // RIFF 奇数长度补 1 字节对齐（不计入长度）
            }
        }
        let mut fmt = Vec::new();
        for n in [1u16, 1] {
            fmt.extend_from_slice(&n.to_le_bytes()); // PCM / 单声道
        }
        for n in [8000u32, 16000] {
            fmt.extend_from_slice(&n.to_le_bytes()); // 采样率 / 字节率
        }
        for n in [2u16, 16] {
            fmt.extend_from_slice(&n.to_le_bytes()); // 块对齐 / 位深
        }
        let mut tag = Vec::new();
        if let Some(png) = embedded_png {
            let mut body = vec![0]; // Latin-1
            body.extend_from_slice(b"image/png\0");
            body.extend_from_slice(&[3, 0]); // PictureType::CoverFront / 空描述
            body.extend_from_slice(png);
            tag.extend_from_slice(b"APIC");
            tag.extend_from_slice(&syncsafe(body.len()));
            tag.extend_from_slice(&[0, 0]);
            tag.extend_from_slice(&body);
        }
        let mut riff = Vec::new();
        push_chunk(&mut riff, b"fmt ", &fmt);
        push_chunk(&mut riff, b"data", &vec![seed; 1600]);
        if !tag.is_empty() {
            let mut id3 = b"ID3\x04\x00\x00".to_vec();
            id3.extend_from_slice(&syncsafe(tag.len()));
            id3.extend_from_slice(&tag);
            push_chunk(&mut riff, b"ID3 ", &id3);
        }
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&(riff.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(&riff);
        wav
    }

    /// 模拟一次扫描（与 CLI `hmp scan` 同序：先落封面再入行），返回初始
    /// `cover_uri`（无封面 None）。
    fn scan_file(db: &mut LibraryDb, root_id: i64, generation: i64, path: &Path) -> Option<String> {
        let size = std::fs::metadata(path).unwrap().len();
        let fp = hmp_storage::scan::file_fingerprint(path, size).unwrap();
        let meta = hmp_storage::read_meta(path);
        let cover = meta
            .as_ref()
            .and_then(|m| m.cover.as_deref())
            .and_then(|c| hmp_storage::scan::persist_cover(c).ok());
        db.record_scan_file(root_id, generation, path, meta.as_ref(), &fp)
            .unwrap();
        if let Some(uri) = &cover {
            db.set_track_cover(&format!("local:{}", path.display()), uri)
                .unwrap();
        }
        cover
    }

    fn cover_of(db: &mut LibraryDb, path: &Path) -> Option<String> {
        let key = format!("local:{}", path.display());
        let rows = db.track_meta_batch("local", &[key]).unwrap();
        assert_eq!(rows.len(), 1, "曲目应已入库: {}", path.display());
        rows.into_iter().next().unwrap().cover_uri
    }

    fn cover_bytes(uri: &str) -> Vec<u8> {
        std::fs::read(uri.strip_prefix("file://").unwrap()).unwrap()
    }

    /// 目录封面语义（直接驱动刷新函数，不依赖 notify 时序）：新增 → 无内嵌
    /// 图的曲目改用目录封面；删除 → 回退内嵌图、无内嵌图清空；内嵌图不被
    /// 目录封面覆盖；大小写变体（`Front.png`）同样识别；其它目录的封面
    /// 不跟着变（不做跨目录清理）。
    #[test]
    fn folder_cover_refresh_updates_only_affected_rows() {
        // 锁中毒（同 suite 其它用例先 panic）不级联：取回内部 guard 继续。
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _data = IsolatedDataDir::new();
        let tmp = tempfile::tempdir().unwrap();
        let root = hmp_storage::canonical_display_path(tmp.path());
        let album = root.join("album");
        let other = root.join("other");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let plain = album.join("plain.wav");
        // 同目录第二首无内嵌图曲目：封面 URI 必须与 plain 一致。
        let plain2 = album.join("plain2.wav");
        let embedded = album.join("embedded.wav");
        let outsider = other.join("outsider.wav");
        std::fs::write(&plain, minimal_wav(1, None)).unwrap();
        std::fs::write(&plain2, minimal_wav(5, None)).unwrap();
        std::fs::write(&embedded, minimal_wav(2, Some(EMBEDDED_PNG))).unwrap();
        std::fs::write(&outsider, minimal_wav(3, None)).unwrap();
        // 邻目录先有自己的目录封面：后续删 album 封面不能把它清掉。
        std::fs::write(other.join("cover.jpg"), FRONT_PNG).unwrap();

        let mut db = LibraryDb::open_in_memory().unwrap();
        let (root_id, generation) = db.begin_scan(&root).unwrap();
        assert_eq!(scan_file(&mut db, root_id, generation, &plain), None);
        assert_eq!(scan_file(&mut db, root_id, generation, &plain2), None);
        let embedded_cover =
            scan_file(&mut db, root_id, generation, &embedded).expect("内嵌图应在扫描时落库");
        let outsider_cover =
            scan_file(&mut db, root_id, generation, &outsider).expect("邻目录封面应在扫描时落库");
        assert_eq!(cover_bytes(&embedded_cover), EMBEDDED_PNG);
        assert_eq!(cover_bytes(&outsider_cover), FRONT_PNG);
        let library = Arc::new(Mutex::new(db));
        let read = |p: &Path| {
            let mut db = library.lock().unwrap();
            cover_of(&mut db, p)
        };
        let refresh = |dir: &Path| LocalWatcher::refresh_folder_cover(&library, dir);

        // 扫描之后才放进 album/cover.jpg：无需重扫，plain 指向新封面。
        std::fs::write(album.join("cover.jpg"), FOLDER_PNG).unwrap();
        assert!(refresh(&album), "新增封面应产生行变化");
        let plain_cover = read(&plain).expect("无内嵌图的曲目应采用目录封面");
        assert_eq!(cover_bytes(&plain_cover), FOLDER_PNG);
        assert_eq!(
            read(&plain2).as_deref(),
            Some(plain_cover.as_str()),
            "同目录无内嵌图的曲目封面一致"
        );
        assert_eq!(
            read(&embedded).as_deref(),
            Some(embedded_cover.as_str()),
            "有内嵌图的曲目不被目录封面覆盖"
        );
        assert_eq!(
            read(&outsider).as_deref(),
            Some(outsider_cover.as_str()),
            "其它目录的封面不受影响"
        );
        // 无变化再触发一次：零写入（返回 false）。
        assert!(!refresh(&album), "封面未变化不应写库");

        // 替换封面（同名文件换内容）→ 两首无内嵌图曲目一起改指向。
        std::fs::write(album.join("cover.jpg"), FRONT_PNG).unwrap();
        assert!(refresh(&album), "替换封面应产生行变化");
        let replaced = read(&plain).expect("替换后的封面应生效");
        assert_eq!(cover_bytes(&replaced), FRONT_PNG);
        assert_eq!(read(&plain2).as_deref(), Some(replaced.as_str()));

        // 删除封面：无内嵌图 → 清空；有内嵌图 → 仍指向内嵌图。
        std::fs::remove_file(album.join("cover.jpg")).unwrap();
        assert!(refresh(&album), "删除封面应产生行变化");
        assert_eq!(read(&plain), None, "无内嵌图且无目录封面 → cover_uri 清空");
        assert_eq!(read(&plain2), None, "同目录行一起清空");
        assert_eq!(
            read(&embedded).as_deref(),
            Some(embedded_cover.as_str()),
            "内嵌图在目录封面删除后仍生效"
        );
        assert_eq!(
            read(&outsider).as_deref(),
            Some(outsider_cover.as_str()),
            "其它目录的封面不跟着清"
        );

        // 大小写变体 Front.png：与探测规则同源，watcher 识别得到。
        std::fs::write(album.join("Front.png"), FOLDER_PNG).unwrap();
        assert!(refresh(&album));
        let front_cover = read(&plain).expect("大小写变体封面应被采用");
        assert_eq!(cover_bytes(&front_cover), FOLDER_PNG);
        assert_eq!(read(&plain2).as_deref(), Some(front_cover.as_str()));
        std::fs::remove_file(album.join("Front.png")).unwrap();
        assert!(refresh(&album));
        assert_eq!(read(&plain), None);
        assert_eq!(read(&plain2), None);
    }

    /// 端到端：watcher 必须把封面文件事件入队（此前只认音频扩展名）。扫描后
    /// 才放进目录的 cover.jpg 无需重扫即生效；删除后清空。
    #[allow(clippy::await_holding_lock)] // 封面落盘目录的 env 隔离，测试内有意跨 await 串行
    #[tokio::test]
    async fn watcher_applies_folder_cover_events() {
        // 锁中毒（同 suite 其它用例先 panic）不级联：取回内部 guard 继续。
        let _env = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _data = IsolatedDataDir::new();
        let music = tempfile::tempdir().unwrap();
        let root = hmp_storage::canonical_display_path(music.path());
        let song = root.join("song.wav");
        std::fs::write(&song, minimal_wav(4, None)).unwrap();
        let library = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        {
            let mut db = library.lock().unwrap();
            let (root_id, generation) = db.begin_scan(&root).unwrap();
            assert_eq!(scan_file(&mut db, root_id, generation, &song), None);
        }
        let (notify, _notify_rx) = tokio::sync::watch::channel(0u64);
        let Some(watcher) = LocalWatcher::spawn(library.clone(), notify) else {
            eprintln!("跳过：环境不支持目录监听");
            return;
        };
        std::fs::write(root.join("cover.jpg"), FOLDER_PNG).unwrap();
        let mut applied = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let uri = {
                let mut db = library.lock().unwrap();
                cover_of(&mut db, &song)
            };
            if let Some(uri) = uri
                && cover_bytes(&uri) == FOLDER_PNG
            {
                applied = true;
                break;
            }
        }
        assert!(applied, "扫描后新增的封面应由 watcher 直接生效（无需重扫）");
        std::fs::remove_file(root.join("cover.jpg")).unwrap();
        let mut cleared = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let uri = {
                let mut db = library.lock().unwrap();
                cover_of(&mut db, &song)
            };
            if uri.is_none() {
                cleared = true;
                break;
            }
        }
        assert!(cleared, "删除封面后 cover_uri 应清空");
        drop(watcher);
    }
}
