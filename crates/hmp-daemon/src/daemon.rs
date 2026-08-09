//! 后端组装：引擎 + 解析器 + 适配器（spec §4.2 `daemon.rs`）。
use std::sync::Arc;

use hmp_qqmusic_api::QqMusicClient;
use hmp_storage::credential::store_from_env;

use crate::engine::{EngineHandle, PlaybackEngine};
use crate::local::{CompositeSourceResolver, LocalSourceResolver};
use crate::player::{GstDriver, PlaybackDriver, QqSourceResolver, SourceResolver};

/// 后端运行配置。
pub struct DaemonConfig {
    /// 测试可传 "fakesink"；None = 系统默认音频输出。
    pub audio_sink: Option<String>,
}

/// 组装后端并返回引擎句柄（服务器/tray/MPRIS 由 Task 3/5/6 接入）。
pub struct Daemon {
    pub handle: EngineHandle,
}

impl Daemon {
    pub fn start(cfg: DaemonConfig) -> Result<Self, hmp_core::HmpError> {
        let driver: Arc<dyn PlaybackDriver> = Arc::new(GstDriver::new(cfg.audio_sink.as_deref())?);
        let store = store_from_env();
        let resolver = Arc::new(QqSourceResolver::new(QqMusicClient::new(), store));
        let credential_ok = {
            let resolver = Arc::clone(&resolver);
            Arc::new(move || resolver.has_credential())
        };
        // 媒体库（`$XDG_DATA_HOME/hmp/library.sqlite3`）；打开失败回退内存库
        // （播放历史不持久，播放不受阻断）。
        let library =
            match hmp_storage::LibraryDb::open(&hmp_storage::data_dir().join("library.sqlite3")) {
                Ok(db) => Arc::new(std::sync::Mutex::new(db)),
                Err(e) => {
                    tracing::warn!(%e, "媒体库打开失败，回退内存库（历史不持久）");
                    Arc::new(std::sync::Mutex::new(
                        hmp_storage::LibraryDb::open_in_memory().unwrap(),
                    ))
                }
            };
        // 组合解析器：QQ（网络取流，需凭证）+ 本地（file://，无需凭证）。
        let local = Arc::new(LocalSourceResolver::new(library.clone()));
        let resolver: Arc<dyn SourceResolver> =
            Arc::new(CompositeSourceResolver::new(resolver, local));
        let handle = PlaybackEngine::start_with_library(
            driver,
            resolver,
            credential_ok,
            Some(library.clone()),
        );
        // 媒体库同步 worker（本地先提交 + QQ 乐观同步；无凭证时离线意图留存）。
        let sync_handle =
            crate::sync::SyncWorker::spawn(library.clone(), QqMusicClient::new(), store_from_env());
        let mut handle = handle;
        handle.library = Some(library);
        handle.sync_handle = Some(sync_handle);
        Ok(Self { handle })
    }
}
