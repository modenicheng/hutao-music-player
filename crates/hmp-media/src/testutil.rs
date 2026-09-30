//! 测试辅助工具：构造 QMC2 加密数据、缓存路径、XDG 隔离等。
//!
//! 供 `decrypt` 与 `stream` 模块测试共用。
//!
//! 仅在 `#[cfg(test)]` 时编译。

use std::path::PathBuf;
use std::sync::Mutex;

use hmp_qqmusic_api::algorithms::qmc2::{decrypt_factory, key::generate_ekey};

/// XDG 环境隔离全局串行锁：`XDG_CACHE_HOME` 是进程级环境变量，凡走
/// `hmp_storage::cache_dir()` 的测试（tee / cache_fill / 回退缓存）必须先
/// 取本锁再改环境，否则并行测试互相踩踏（lib.rs 与 stream 测试共享本锁）。
pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

/// 构造 QMC2 加密测试数据。
///
/// - `plaintext`：明文音频数据（应以已知魔数开头，如 `b"fLaC"`）
/// - `key`：原始密钥字节
/// - `with_footer`：是否在末尾附加 V1 尾部 `[key_bytes][key_len LE u32]`
///
/// 返回 `(encrypted_data, ekey)`。
pub(crate) fn make_encrypted(plaintext: &[u8], key: &[u8], with_footer: bool) -> (Vec<u8>, String) {
    let ekey = generate_ekey(key);
    let cipher = decrypt_factory(&ekey).unwrap();

    let mut encrypted = plaintext.to_vec();
    cipher.decrypt(0, &mut encrypted);

    if with_footer {
        let key_len = key.len() as u32;
        encrypted.extend_from_slice(key);
        encrypted.extend_from_slice(&key_len.to_le_bytes());
    }

    (encrypted, ekey)
}

/// 为当前进程创建唯一的临时缓存根目录路径。
pub(crate) fn test_cache_root() -> PathBuf {
    std::env::temp_dir().join(format!("hmp-media-test-{}", std::process::id()))
}

/// 隔离 `XDG_CACHE_HOME` 至独立临时目录（测试结束自动清理），
/// 并持有全局串行锁直到返回值 drop。
pub(crate) fn isolate_cache(name: &str) -> TestCacheIsolation {
    // 测试 panic 会毒化锁；锁只做互斥用，毒化无意义，直接恢复
    let lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = std::env::temp_dir().join(format!("hmp-media-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    unsafe {
        std::env::set_var("XDG_CACHE_HOME", &root);
    }
    TestCacheIsolation {
        _env: CacheEnvRestore { root: root.clone() },
        _lock: lock,
    }
}

/// XDG 隔离生命周期守卫（env 恢复先于锁释放）。
pub(crate) struct TestCacheIsolation {
    _env: CacheEnvRestore,
    _lock: std::sync::MutexGuard<'static, ()>,
}

struct CacheEnvRestore {
    root: PathBuf,
}

impl Drop for CacheEnvRestore {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("XDG_CACHE_HOME");
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 解析 wiremock 收到的全部 GET Range 请求（排除探测 `bytes=0-0`），
/// 返回 `(start, end)` 列表（按到达顺序）。
pub(crate) async fn range_gets(server: &wiremock::MockServer) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for req in server.received_requests().await.unwrap_or_default() {
        if req.method != "GET" {
            continue;
        }
        let Some(v) = req.headers.get("range").and_then(|v| v.to_str().ok()) else {
            continue;
        };
        let Some(spec) = v.strip_prefix("bytes=") else {
            continue;
        };
        let Some((s, e)) = spec.split_once('-') else {
            continue;
        };
        let (Ok(s), Ok(e)) = (s.parse::<u64>(), e.parse::<u64>()) else {
            continue;
        };
        if s == 0 && e == 0 {
            continue; // Range 支持探测
        }
        out.push((s, e));
    }
    out
}

/// 数据区间 GET 计数：排除 `bytes=0-0` 探测与尾部探测（`start >= tail_from`）。
pub(crate) async fn data_get_count(server: &wiremock::MockServer, tail_from: u64) -> usize {
    range_gets(server)
        .await
        .iter()
        .filter(|(s, _)| *s < tail_from)
        .count()
}

/// 轮询等待条件成立（25ms 间隔，默认 10s 超时 panic 并附 `what` 说明）。
pub(crate) async fn eventually<T>(what: &str, mut cond: impl FnMut() -> Option<T>) -> T {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(v) = cond() {
            return v;
        }
        if std::time::Instant::now() > deadline {
            panic!("等待超时: {what}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// [`eventually`] 的异步条件版（如 wiremock 请求计数）。
pub(crate) async fn eventually_async<T, F, Fut>(what: &str, mut cond: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(v) = cond().await {
            return v;
        }
        if std::time::Instant::now() > deadline {
            panic!("等待超时: {what}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// 在 blocking 线程运行消费者操作：reader 的 `read` 在数据饥饿时阻塞于
/// condvar，测试（current_thread runtime）不能在 runtime 线程上阻塞，
/// 否则生产者任务被饿死死锁。
pub(crate) async fn blocking<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await.unwrap()
}
