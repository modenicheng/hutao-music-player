//! 歌词磁盘缓存（L2）：`<cache_dir>/lyrics/<mid>.lrc` + `<mid>.trans.lrc`。
//!
//! daemon 歌词读取的三级结构：内存 TTL（L1，进程内）→ 本模块（L2，跨重启）
//! → QQ 网络。持久目录经 [`crate::cache_dir`]（Linux `~/.cache/hmp`、
//! Windows `%LOCALAPPDATA%\hmp\cache`），不落 /tmp。
//!
//! 只缓存非空正文（空结果不落盘，负缓存仍仅内存）；mid 白名单校验防路径注入。

use std::path::PathBuf;

/// 歌词缓存目录：`<cache_dir>/lyrics`。
pub fn lyric_cache_dir() -> PathBuf {
    crate::cache_dir().join("lyrics")
}

/// 读取缓存的歌词，返回 `(正文, 翻译)`；无缓存或正文为空 → None。
pub fn read_cached_lyric(mid: &str) -> Option<(String, String)> {
    if !valid_mid(mid) {
        return None;
    }
    let lyric = std::fs::read_to_string(lyric_cache_dir().join(format!("{mid}.lrc"))).ok()?;
    if lyric.trim().is_empty() {
        return None;
    }
    let translation = std::fs::read_to_string(lyric_cache_dir().join(format!("{mid}.trans.lrc")))
        .unwrap_or_default();
    Some((lyric, translation))
}

/// 写入歌词缓存（原子 tmp+rename，persist_cover 同款；翻译可为空 = 不写译文文件）。
/// 失败仅返回 Err 由调用方记日志（缓存写失败不阻断播放）。
pub fn write_cached_lyric(mid: &str, lyric: &str, translation: &str) -> std::io::Result<()> {
    if !valid_mid(mid) || lyric.trim().is_empty() {
        return Ok(());
    }
    let dir = lyric_cache_dir();
    std::fs::create_dir_all(&dir)?;
    write_atomic(&dir.join(format!("{mid}.lrc")), lyric)?;
    if !translation.trim().is_empty() {
        write_atomic(&dir.join(format!("{mid}.trans.lrc")), translation)?;
    }
    Ok(())
}

fn write_atomic(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    let mut tmp_name = name.clone();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// mid 白名单：QQ songmid 为字母数字（防御性：拒绝路径分隔符与 `..`）。
fn valid_mid(mid: &str) -> bool {
    !mid.is_empty() && mid.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 隔离守卫（xdg.rs TempGuard 同款）：持 crate 级 TEST_ENV_LOCK
    /// 贯穿整个测试体（xdg.rs 测试同样改写 XDG_CACHE_HOME，无锁互踩），
    /// Drop 时恢复 env 并清理临时目录。
    struct IsolatedCacheDir {
        root: std::path::PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl IsolatedCacheDir {
        fn new(tag: &str) -> Self {
            let _lock = crate::TEST_ENV_LOCK.lock().unwrap();
            let root = std::env::temp_dir().join(format!("hmp-lyric-cache-test-{tag}"));
            let _ = std::fs::remove_dir_all(&root);
            unsafe {
                std::env::set_var("XDG_CACHE_HOME", &root);
            }
            Self { root, _lock }
        }
    }
    impl Drop for IsolatedCacheDir {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("XDG_CACHE_HOME");
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn write_then_read_roundtrip() {
        let _cache = IsolatedCacheDir::new("roundtrip");
        write_cached_lyric("0039MnYb0qxYhV", "[00:01.00]正文\n", "[00:01.00]译文\n").unwrap();
        let (lyric, trans) = read_cached_lyric("0039MnYb0qxYhV").unwrap();
        assert_eq!(lyric, "[00:01.00]正文\n");
        assert_eq!(trans, "[00:01.00]译文\n");

        // 无译文文件时译文为空串而非 Err
        write_cached_lyric("aaaabbbbcccc", "[00:02.00]无译文\n", "").unwrap();
        let (lyric, trans) = read_cached_lyric("aaaabbbbcccc").unwrap();
        assert_eq!(lyric, "[00:02.00]无译文\n");
        assert_eq!(trans, "");

        // 未写过 → None
        assert!(read_cached_lyric("nonexistent00").is_none());
    }

    #[test]
    fn rejects_invalid_mid_and_empty_body() {
        let cache = IsolatedCacheDir::new("reject");
        let _ = cache; // 守卫贯穿测试体
        for bad in ["../evil", "a/b", "", "a b", "a\\b"] {
            assert!(read_cached_lyric(bad).is_none(), "{bad} 应拒绝");
            assert!(
                write_cached_lyric(bad, "x", "").is_ok(),
                "{bad} 写入应静默跳过"
            );
        }
        // 空正文不落盘
        write_cached_lyric("valid0mid", "   \n", "").unwrap();
        assert!(read_cached_lyric("valid0mid").is_none());
    }
}
