//! XDG 基础目录（XDG Base Directory Specification）。
//!
//! Windows 走 Known Folders（`%APPDATA%`/`%LOCALAPPDATA%`），**有意忽略
//! HOME**：msys/git-bash 终端会给子进程注入 Linux 形态的 HOME，曾让终端
//! 拉起的 daemon 用 `~/.local/share` 而 GUI/惯例线用 `AppData\Local`——
//! 同一用户两套媒体库、两套播放状态（2026-09-09 Windows 排查事故）。
//! 显式 `XDG_*_HOME` 覆盖在两平台都保留（测试/便携化场景）。

use std::path::PathBuf;

#[cfg(unix)]
fn from_env_or_home(env: &str, fallback_dir: &str) -> PathBuf {
    if let Some(v) = std::env::var_os(env) {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    // HOME 是 XDG 规范回退。
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/tmp".into());
    PathBuf::from(home).join(fallback_dir)
}

/// Windows Known-Folder 解析：`XDG_*_HOME` 显式覆盖 → `%folder_var%` →
/// `USERPROFILE\AppData\<profile_part>` → temp 兜底（无 USERPROFILE 的
/// 极端服务环境）。
#[cfg(windows)]
fn windows_dir(xdg_var: &str, folder_var: &str, profile_part: &str) -> PathBuf {
    if let Some(v) = std::env::var_os(xdg_var) {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Some(v) = std::env::var_os(folder_var) {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    std::env::var_os("USERPROFILE")
        .filter(|v| !v.is_empty())
        .map(|p| PathBuf::from(p).join("AppData").join(profile_part))
        .unwrap_or_else(|| std::env::temp_dir().join("hmp"))
}

/// 配置目录：Unix `$XDG_CONFIG_HOME/hmp`（默认 `~/.config/hmp`）；
/// Windows `%APPDATA%\hmp`（Roaming）。
pub fn config_dir() -> PathBuf {
    #[cfg(unix)]
    {
        from_env_or_home("XDG_CONFIG_HOME", ".config").join("hmp")
    }
    #[cfg(windows)]
    {
        windows_dir("XDG_CONFIG_HOME", "APPDATA", "Roaming").join("hmp")
    }
}

/// 数据目录：Unix `$XDG_DATA_HOME/hmp`（默认 `~/.local/share/hmp`）；
/// Windows `%LOCALAPPDATA%\hmp`。
pub fn data_dir() -> PathBuf {
    #[cfg(unix)]
    {
        from_env_or_home("XDG_DATA_HOME", ".local/share").join("hmp")
    }
    #[cfg(windows)]
    {
        windows_dir("XDG_DATA_HOME", "LOCALAPPDATA", "Local").join("hmp")
    }
}

/// 缓存目录：Unix `$XDG_CACHE_HOME/hmp`（默认 `~/.cache/hmp`）；
/// Windows `%LOCALAPPDATA%\hmp\cache`（XDG 覆盖时同 Unix 语义 `$VAR/hmp`）。
pub fn cache_dir() -> PathBuf {
    #[cfg(unix)]
    {
        from_env_or_home("XDG_CACHE_HOME", ".cache").join("hmp")
    }
    #[cfg(windows)]
    {
        if let Some(v) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(v).join("hmp");
        }
        if let Some(v) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
            return PathBuf::from(v).join("hmp").join("cache");
        }
        std::env::var_os("USERPROFILE")
            .filter(|v| !v.is_empty())
            .map(|p| {
                PathBuf::from(p)
                    .join("AppData")
                    .join("Local")
                    .join("hmp")
                    .join("cache")
            })
            .unwrap_or_else(|| std::env::temp_dir().join("hmp"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TEST_ENV_LOCK;

    #[test]
    fn dirs_follow_env_override() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
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

    /// Unix HOME 回退（HOME 是 XDG 规范回退）。
    #[cfg(unix)]
    #[test]
    fn dirs_fallback_to_home() {
        let _lock = TEST_ENV_LOCK.lock().unwrap();
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

    /// Windows Known Folders：config=Roaming、data/cache=Local，cache 在
    /// hmp 下再分 cache 子目录。LOCALAPPDATA/APPDATA 均以 XDG 覆盖为空、
    /// 真实值缺失为前提显式设定。
    #[cfg(windows)]
    #[test]
    fn dirs_map_to_windows_known_folders() {
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        let guard = TempGuard::new();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var("XDG_DATA_HOME");
            std::env::remove_var("XDG_CACHE_HOME");
            std::env::set_var("APPDATA", r"C:\t\roam");
            std::env::set_var("LOCALAPPDATA", r"C:\t\local");
        }
        assert_eq!(config_dir(), PathBuf::from(r"C:\t\roam\hmp"));
        assert_eq!(data_dir(), PathBuf::from(r"C:\t\local\hmp"));
        assert_eq!(cache_dir(), PathBuf::from(r"C:\t\local\hmp\cache"));
        guard.restore();
    }

    /// msys/git-bash 注入的 HOME 在 Windows 上必须被忽略（双数据目录事故
    /// 的回归守护）：LOCALAPPDATA 存在时 HOME 不参与解析。
    #[cfg(windows)]
    #[test]
    fn windows_dirs_ignore_msys_home() {
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        let guard = TempGuard::new();
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
            std::env::set_var("HOME", r"C:\Users\bogus");
            std::env::set_var("LOCALAPPDATA", r"C:\t\local");
        }
        assert_eq!(data_dir(), PathBuf::from(r"C:\t\local\hmp"));
        guard.restore();
    }

    /// Windows 极端环境：APPDATA/LOCALAPPDATA 缺失 → USERPROFILE 拼装。
    #[cfg(windows)]
    #[test]
    fn windows_dirs_fall_back_to_userprofile() {
        let _lock = TEST_ENV_LOCK.lock().unwrap();
        let guard = TempGuard::new();
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
            std::env::remove_var("LOCALAPPDATA");
            std::env::set_var("USERPROFILE", r"C:\Users\u");
        }
        assert_eq!(data_dir(), PathBuf::from(r"C:\Users\u\AppData\Local\hmp"));
        guard.restore();
    }

    /// 保存并恢复 XDG/HOME/Windows Known Folder 环境变量。
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
                std::env::remove_var("USERPROFILE");
                std::env::remove_var("APPDATA");
                std::env::remove_var("LOCALAPPDATA");
            }
        }
    }
}
