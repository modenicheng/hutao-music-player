//! 库数据隔离回归：离线入口不得打开真实 data_dir。
//!
//! 本二进制只有这一个用例，启动时先把 `XDG_DATA_HOME` 指向临时目录，并在
//! 临时库里播种一条「磁盘行」与一张本地专辑；随后只用注入快照 + 离线绑定
//! 启动 UI。若 `load_data_with_snapshot` / `bind_offline` 回退成读真实库，
//! 注入断言或 mtime 断言即红。
//!
//! 选进程级目录隔离而非只看模型，是为了让「有没有碰库」可观测：临时库既
//! 不是真实用户数据，又能被 SQLite 正常打开，回退实现会在这里留下痕迹。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use slint::{ComponentHandle, Global, Model};

use hmp_desktop::library_view::{Snapshot, SongRow};
use hmp_desktop::{Data, Nav, Route};

/// 进程级数据目录隔离。
///
/// SAFETY：本二进制仅一个 `#[test]`；调用点在 testing backend / runtime / UI
/// 线程创建之前，`OnceLock` 保证进程内只写一次，写后不再改动环境变量——
/// 没有其它线程会并发读取环境，满足 Rust 2024 `set_var` 的 unsafe 前提。
fn isolated_data_home() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root =
            std::env::temp_dir().join(format!("hmp-desktop-library-iso-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("hmp")).unwrap();
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &root);
            std::env::set_var("HMP_IPC_ENDPOINT", bogus_endpoint(&root));
        }
        root
    })
}

fn bogus_endpoint(root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = root;
        PathBuf::from(r"\\.\pipe\hmp-test-library-iso-bogus")
    }
    #[cfg(unix)]
    {
        root.join("hmp-no-daemon.sock")
    }
}

/// 磁盘态指纹：长度 + mtime；文件不存在 → None。
fn file_stamp(path: &Path) -> Option<(u64, SystemTime)> {
    std::fs::metadata(path)
        .ok()
        .map(|m| (m.len(), m.modified().unwrap()))
}

/// 临时库的播种：一条 QQ 收藏（磁盘行）+ 一张本地专辑。模拟真实库里
/// 「离线入口不该看到」的内容。
fn seed_disk_library(db_path: &Path, root: &Path) {
    let mut db = hmp_storage::LibraryDb::open(db_path).expect("临时库可建");
    db.upsert_track(&hmp_storage::TrackRow {
        source: "qq",
        source_key: "disk-mid".into(),
        title: "磁盘行".into(),
        ..Default::default()
    })
    .unwrap();
    db.add_favorite("qq", "disk-mid", "磁盘行").unwrap();

    let audio = root.join("audio/iso.flac");
    std::fs::create_dir_all(audio.parent().unwrap()).unwrap();
    std::fs::write(&audio, b"not-real-audio").unwrap();
    db.add_local_file(
        &audio,
        Some(&hmp_storage::LocalMeta {
            title: "隔离曲".into(),
            artist: Some("隔离歌手".into()),
            album: Some("隔离专辑".into()),
            duration_ms: Some(1_000),
            ..Default::default()
        }),
    )
    .unwrap();
}

#[test]
fn injected_snapshot_and_offline_bind_never_touch_data_dir() {
    let root = isolated_data_home();
    i_slint_backend_testing::init_no_event_loop();

    let db_path = hmp_storage::data_dir().join("library.sqlite3");
    assert!(
        db_path.starts_with(root),
        "本测试必须运行在临时 data_dir，实际 {db_path:?}"
    );
    seed_disk_library(&db_path, root);
    let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
    let shm_path = PathBuf::from(format!("{}-shm", db_path.display()));
    let db_before = file_stamp(&db_path);
    let wal_before = file_stamp(&wal_path);
    let shm_before = file_stamp(&shm_path);

    let ui = hmp_desktop::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let runtime = Arc::new(hmp_desktop::backend::BackendRuntime::new().unwrap());

    // 注入快照：内容与临时库完全不同。真实快照投影会得到「磁盘行」。
    let injected = Snapshot {
        liked: vec![SongRow {
            mid: "injected-mid".into(),
            source: 0,
            title: "注入行".into(),
            artists: "注入歌手".into(),
            album: "注入专辑".into(),
            duration_ms: 1_000,
            quality: String::new(),
            cover_uri: None,
            remote_cover: None,
            folder: None,
        }],
        ..Snapshot::empty()
    };
    hmp_desktop::bridge::load_data_with_snapshot(&ui, &runtime, &injected);

    let data = Data::get(&ui);
    assert_eq!(data.get_liked_count(), 1, "只能装载注入快照");
    let liked = data.get_liked();
    assert_eq!(liked.row_count(), 1);
    assert_eq!(liked.row_data(0).unwrap().title.as_str(), "注入行");
    assert!(
        !liked.iter().any(|row| row.title == "磁盘行"),
        "注入快照不得混入磁盘库内容"
    );

    // 离线绑定：导航到临时库里真实存在的专辑，也不得命中。
    let prefs = Arc::new(Mutex::new(hmp_desktop::prefs::Prefs::default()));
    hmp_desktop::bridge::bind_offline(&ui, prefs, Arc::clone(&runtime));
    let nav = Nav::get(&ui);
    nav.invoke_navigate(Route::Album, "隔离专辑".into());
    assert_eq!(nav.get_route(), Route::Album);
    assert_eq!(data.get_album_state(), 3, "离线绑定应命中「未找到」空态");
    assert!(!data.get_album_found());
    assert_ne!(data.get_album_name().as_str(), "隔离专辑");

    // 磁盘指纹不变：主库、WAL、SHM 都不能被打开/改写。
    assert_eq!(file_stamp(&db_path), db_before, "主库被触碰");
    assert_eq!(file_stamp(&wal_path), wal_before, "WAL 被创建/改写");
    assert_eq!(file_stamp(&shm_path), shm_before, "SHM 被创建/改写");

    let _ = std::fs::remove_dir_all(root);
}
