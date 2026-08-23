//! 跨平台应用目录；显式的 XDG 环境变量始终优先。

use std::path::PathBuf;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(not(windows))]
fn home_base(fallback_dir: &str) -> PathBuf {
    env_path("HOME")
        .unwrap_or_else(std::env::temp_dir)
        .join(fallback_dir)
}

#[cfg(windows)]
fn windows_base(preferred_env: &str, profile_suffix: &str) -> PathBuf {
    env_path(preferred_env)
        .or_else(|| env_path("USERPROFILE").map(|path| path.join(profile_suffix)))
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(windows)]
fn default_config_base() -> PathBuf {
    windows_base("APPDATA", "AppData/Roaming")
}

#[cfg(not(windows))]
fn default_config_base() -> PathBuf {
    home_base(".config")
}

#[cfg(windows)]
fn default_data_base() -> PathBuf {
    windows_base("LOCALAPPDATA", "AppData/Local")
}

#[cfg(not(windows))]
fn default_data_base() -> PathBuf {
    home_base(".local/share")
}

#[cfg(windows)]
fn default_cache_base() -> PathBuf {
    windows_base("LOCALAPPDATA", "AppData/Local")
        .join("hmp")
        .join("cache")
}

#[cfg(not(windows))]
fn default_cache_base() -> PathBuf {
    home_base(".cache").join("hmp")
}

/// 配置目录（Windows 为 `%APPDATA%\hmp`，Unix 为 `~/.config/hmp`）。
pub fn config_dir() -> PathBuf {
    env_path("XDG_CONFIG_HOME")
        .unwrap_or_else(default_config_base)
        .join("hmp")
}

/// 数据目录（Windows 为 `%LOCALAPPDATA%\hmp`，Unix 为 `~/.local/share/hmp`）。
pub fn data_dir() -> PathBuf {
    env_path("XDG_DATA_HOME")
        .unwrap_or_else(default_data_base)
        .join("hmp")
}

/// 缓存目录（Windows 为 `%LOCALAPPDATA%\hmp\cache`，Unix 为 `~/.cache/hmp`）。
pub fn cache_dir() -> PathBuf {
    env_path("XDG_CACHE_HOME")
        .map(|path| path.join("hmp"))
        .unwrap_or_else(default_cache_base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TEST_ENV_LOCK;

    #[test]
    fn dirs_follow_env_override() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::new(&["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"]);
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "/tmp/hmp-test-config");
            std::env::set_var("XDG_DATA_HOME", "/tmp/hmp-test-data");
            std::env::set_var("XDG_CACHE_HOME", "/tmp/hmp-test-cache");
        }
        assert_eq!(config_dir(), PathBuf::from("/tmp/hmp-test-config/hmp"));
        assert_eq!(data_dir(), PathBuf::from("/tmp/hmp-test-data/hmp"));
        assert_eq!(cache_dir(), PathBuf::from("/tmp/hmp-test-cache/hmp"));
    }

    #[cfg(not(windows))]
    #[test]
    fn dirs_fallback_to_home() {
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::new(&["XDG_CONFIG_HOME", "HOME"]);
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::set_var("HOME", "/tmp/hmp-test-home");
        }
        assert_eq!(
            config_dir(),
            PathBuf::from("/tmp/hmp-test-home/.config/hmp")
        );
    }

    #[cfg(windows)]
    #[test]
    fn dirs_use_native_windows_roaming_and_local_bases() {
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::new(&[
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "APPDATA",
            "LOCALAPPDATA",
        ]);
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var("XDG_DATA_HOME");
            std::env::remove_var("XDG_CACHE_HOME");
            std::env::set_var("APPDATA", r"C:\Users\hmp\AppData\Roaming");
            std::env::set_var("LOCALAPPDATA", r"C:\Users\hmp\AppData\Local");
        }

        assert_eq!(
            config_dir(),
            PathBuf::from(r"C:\Users\hmp\AppData\Roaming\hmp")
        );
        assert_eq!(data_dir(), PathBuf::from(r"C:\Users\hmp\AppData\Local\hmp"));
        assert_eq!(
            cache_dir(),
            PathBuf::from(r"C:\Users\hmp\AppData\Local\hmp\cache")
        );
    }

    /// 保存并恢复测试期间修改的环境变量。
    struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl EnvGuard {
        fn new(names: &[&'static str]) -> Self {
            Self(
                names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            )
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.0 {
                match value {
                    Some(value) => unsafe { std::env::set_var(name, value) },
                    None => unsafe { std::env::remove_var(name) },
                }
            }
        }
    }
}
