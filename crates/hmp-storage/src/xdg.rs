//! XDG 基础目录（XDG Base Directory Specification）。

use std::path::PathBuf;

fn from_env_or_home(env: &str, fallback_dir: &str) -> PathBuf {
    if let Some(v) = std::env::var_os(env) {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(home).join(fallback_dir)
}

/// 配置目录（`$XDG_CONFIG_HOME/hmp`，默认 `~/.config/hmp`）。
pub fn config_dir() -> PathBuf {
    from_env_or_home("XDG_CONFIG_HOME", ".config").join("hmp")
}

/// 数据目录（`$XDG_DATA_HOME/hmp`，默认 `~/.local/share/hmp`）。
pub fn data_dir() -> PathBuf {
    from_env_or_home("XDG_DATA_HOME", ".local/share").join("hmp")
}

/// 缓存目录（`$XDG_CACHE_HOME/hmp`，默认 `~/.cache/hmp`）。
pub fn cache_dir() -> PathBuf {
    from_env_or_home("XDG_CACHE_HOME", ".cache").join("hmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirs_follow_env_override() {
        let guard = TempGuard::new();
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "/tmp/hmp-test-config");
            std::env::set_var("XDG_DATA_HOME", "/tmp/hmp-test-data");
            std::env::set_var("XDG_CACHE_HOME", "/tmp/hmp-test-cache");
        }
        assert_eq!(config_dir(), PathBuf::from("/tmp/hmp-test-config/hmp"));
        assert_eq!(data_dir(), PathBuf::from("/tmp/hmp-test-data/hmp"));
        assert_eq!(cache_dir(), PathBuf::from("/tmp/hmp-test-cache/hmp"));
        guard.restore();
    }

    #[test]
    fn dirs_fallback_to_home() {
        let guard = TempGuard::new();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::set_var("HOME", "/tmp/hmp-test-home");
        }
        assert_eq!(
            config_dir(),
            PathBuf::from("/tmp/hmp-test-home/.config/hmp")
        );
        guard.restore();
    }

    /// 保存并恢复 XDG/HOME 环境变量。
    struct TempGuard;
    impl TempGuard {
        fn new() -> Self {
            TempGuard
        }
        fn restore(&self) {
            unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
                std::env::remove_var("XDG_DATA_HOME");
                std::env::remove_var("XDG_CACHE_HOME");
                std::env::remove_var("HOME");
            }
        }
    }
}
