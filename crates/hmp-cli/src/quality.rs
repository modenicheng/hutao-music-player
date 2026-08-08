//! `hmp quality`：音质策略查看/设置（持久化到 `~/.config/hmp/config.toml`）。
//!
//! 音质属于 **source resolution policy**：resolver 依据偏好生成回退链，
//! 不改变播放器状态机命令。

use std::io::Write;

use hmp_core::AudioQuality;
use hmp_storage::{Config, QualityMode, QualityPref};

/// 展示当前策略（无参数）。
pub fn format_current() -> String {
    let c = Config::load();
    format!(
        "音质策略: {}\n配置文件: {}",
        c.quality.describe(),
        Config::path().display()
    )
}

/// 设置音质（别名 + 可选禁止回退）。
pub fn set(alias: &str, fallback: bool) -> Result<String, String> {
    let mode = if alias.eq_ignore_ascii_case("auto") {
        QualityMode::Auto
    } else {
        let q = AudioQuality::from_alias(alias).ok_or_else(|| {
            format!("未知音质 `{alias}`（auto|master|hires|atmos|flac|aac|320|128）")
        })?;
        QualityMode::Fixed(q)
    };
    let pref = QualityPref::from_mode(mode, fallback);
    let config = Config { quality: pref };
    config.save().map_err(|e| format!("写入配置失败: {e}"))?;
    Ok(format!(
        "已设置: {}\n生效链: {}",
        config.quality.describe(),
        config
            .quality
            .chain()
            .iter()
            .map(|q| q.to_alias())
            .collect::<Vec<_>>()
            .join(" → ")
    ))
}

/// 运行入口。
pub async fn run(
    alias: Option<String>,
    no_fallback: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let out = match alias {
        Some(alias) => set(&alias, !no_fallback)?,
        None => format_current(),
    };
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{out}")?;
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 串行 + 隔离 XDG_CONFIG_HOME（避免污染真实配置）。
    static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn isolated<T>(f: impl FnOnce() -> T) -> T {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.path());
        }
        let out = f();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        out
    }

    #[test]
    fn set_parses_aliases() {
        isolated(|| {
            assert!(set("flac", true).unwrap().contains("flac"));
            assert!(set("320", true).unwrap().contains("320"));
            assert!(set("auto", true).unwrap().contains("自动"));
            assert!(set("master", false).unwrap().contains("不回退"));
            assert!(set("bogus", true).is_err());
        });
    }

    #[test]
    fn describe_mentions_chain() {
        let c = Config::default();
        assert!(c.quality.describe().contains("master"));
    }
}
