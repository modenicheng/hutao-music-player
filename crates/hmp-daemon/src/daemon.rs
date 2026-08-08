//! 后端组装：引擎 + 解析器 + 适配器（spec §4.2 `daemon.rs`）。
use std::sync::Arc;

use hmp_qqmusic_api::QqMusicClient;
use hmp_storage::credential::store_from_env;

use crate::engine::{EngineHandle, PlaybackEngine};
use crate::player::{GstDriver, PlaybackDriver, QqSourceResolver};

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
        let handle = PlaybackEngine::start(driver, resolver, credential_ok);
        Ok(Self { handle })
    }
}
