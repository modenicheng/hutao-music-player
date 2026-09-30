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
        "Quality policy: {}\nConfig file: {}",
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
            format!("unknown quality `{alias}` (auto|master|hires|atmos|flac|aac|320|128)")
        })?;
        QualityMode::Fixed(q)
    };
    let pref = QualityPref::from_mode(mode, fallback);
    // 保留既有配置（如 [audio] sink）：只改 quality 字段，不整体重建（里程碑 G）。
    let mut config = Config::load();
    config.quality = pref;
    config
        .save()
        .map_err(|e| format!("failed to write config: {e}"))?;
    Ok(format!(
        "Set: {}\nEffective chain: {}",
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
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(alias) = alias {
        let message = set(&alias, !no_fallback)?;
        if json {
            let config = Config::load();
            return super::output::print(&serde_json::json!({
                "mode": config.quality.mode,
                "fallback": config.quality.fallback,
                "describe": config.quality.describe(),
                "chain": config.quality.chain().iter().map(|q| q.to_alias()).collect::<Vec<_>>(),
                "message": message,
            }));
        }
        println!("{message}");
        return Ok(());
    }
    let config = Config::load();
    if json {
        return super::output::print(&serde_json::json!({
            "mode": config.quality.mode,
            "fallback": config.quality.fallback,
            "describe": config.quality.describe(),
            "chain": config.quality.chain().iter().map(|q| q.to_alias()).collect::<Vec<_>>(),
            "config_path": Config::path().display().to_string(),
        }));
    }
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", format_current())?;
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
            assert!(set("auto", true).unwrap().contains("auto"));
            assert!(set("master", false).unwrap().contains("no fallback"));
            assert!(set("bogus", true).is_err());
        });
    }

    #[test]
    fn set_preserves_audio_preferences() {
        isolated(|| {
            // 预置 ReplayGain 偏好，quality set 不应抹掉。
            let mut c = Config::load();
            c.audio.replaygain = false;
            c.save().unwrap();
            set("flac", true).unwrap();
            let back = Config::load();
            assert!(!back.audio.replaygain);
            assert_eq!(back.quality.mode, "flac");
        });
    }

    #[test]
    fn describe_mentions_chain() {
        let c = Config::default();
        assert!(c.quality.describe().contains("master"));
    }
}
