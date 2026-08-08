//! 持久化偏好配置（`config_dir()/config.toml`）。
//!
//! 第一版只承载音质策略（媒体库重构计划 B2）。音质属于 **source
//! resolution policy** 而非播放器状态机命令：resolver 依据 `QualityPref`
//! 生成回退链，不改变 GStreamer 参数。

use std::path::PathBuf;

use hmp_core::AudioQuality;
use serde::{Deserialize, Serialize};

use crate::xdg::config_dir;

/// 音质模式：自动（从最高档起）或固定档位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QualityMode {
    Auto,
    Fixed(AudioQuality),
}

/// 音质偏好（持久化形态：字符串别名，便于人工编辑 config.toml）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QualityPref {
    /// `"auto"` 或音质别名（`master`/`hires`/`atmos`/`flac`/`aac`/`320`/`128`）。
    #[serde(default = "default_mode_string")]
    pub mode: String,
    /// 是否允许向下降级回退（false = 只尝试指定档位）。
    #[serde(default = "default_true")]
    pub fallback: bool,
}

fn default_mode_string() -> String {
    "auto".into()
}
fn default_true() -> bool {
    true
}

impl Default for QualityPref {
    fn default() -> Self {
        Self {
            mode: default_mode_string(),
            fallback: true,
        }
    }
}

impl QualityPref {
    /// 构建偏好（CLI 写入路径）。
    pub fn from_mode(mode: QualityMode, fallback: bool) -> Self {
        let mode = match mode {
            QualityMode::Auto => "auto".into(),
            QualityMode::Fixed(q) => q.to_alias(),
        };
        Self { mode, fallback }
    }

    /// 解析为模式。
    pub fn mode_enum(&self) -> QualityMode {
        if self.mode == "auto" {
            QualityMode::Auto
        } else {
            match AudioQuality::from_alias(&self.mode) {
                Some(q) => QualityMode::Fixed(q),
                None => QualityMode::Auto, // 配置损坏 → 自动
            }
        }
    }

    /// 生效回退链（resolver 依此尝试音质档位）。
    pub fn chain(&self) -> Vec<AudioQuality> {
        let chain = match self.mode_enum() {
            QualityMode::Auto => AudioQuality::Master.fallback_chain(),
            QualityMode::Fixed(q) => q.fallback_chain(),
        };
        if self.fallback {
            chain
        } else {
            chain.into_iter().take(1).collect()
        }
    }

    /// 人类可读描述（CLI 展示）。
    pub fn describe(&self) -> String {
        match self.mode_enum() {
            QualityMode::Auto => {
                let chain = self
                    .chain()
                    .iter()
                    .map(|q| q.to_alias())
                    .collect::<Vec<_>>()
                    .join("→");
                if self.fallback {
                    format!("自动：{chain}")
                } else {
                    format!("自动（仅最高档：{chain}）")
                }
            }
            QualityMode::Fixed(q) => {
                if self.fallback {
                    let rest = self
                        .chain()
                        .iter()
                        .skip(1)
                        .map(|x| x.to_alias())
                        .collect::<Vec<_>>()
                        .join("/");
                    format!("{}（回退 {rest}）", q.to_alias())
                } else {
                    format!("{}（不回退）", q.to_alias())
                }
            }
        }
    }
}

/// 顶层配置。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub quality: QualityPref,
}

impl Config {
    /// 配置文件路径（`$XDG_CONFIG_HOME/hmp/config.toml`）。
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    /// 读取配置；文件缺失/损坏 → 默认值（不报错）。
    pub fn load() -> Self {
        let Ok(text) = std::fs::read_to_string(Self::path()) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_default()
    }

    /// 原子写配置（临时文件 + rename）。
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在隔离的 XDG_CONFIG_HOME 下运行（与其它改 env 的测试串行）。
    fn with_isolated_config<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
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
    fn default_is_auto_with_fallback() {
        let c = Config::default();
        assert_eq!(c.quality.mode, "auto");
        assert!(c.quality.fallback);
        assert_eq!(c.quality.mode_enum(), QualityMode::Auto);
    }

    #[test]
    fn auto_chain_includes_atmos() {
        let c = Config::default();
        let chain = c.quality.chain();
        assert_eq!(chain.len(), 6);
        assert_eq!(chain[0], AudioQuality::Master);
        assert!(chain.contains(&AudioQuality::Atmos));
        assert_eq!(chain[5], AudioQuality::Mp3_128);
    }

    #[test]
    fn fixed_flac_chain_degrades() {
        let q = QualityPref::from_mode(QualityMode::Fixed(AudioQuality::Flac), true);
        assert_eq!(
            q.chain(),
            vec![
                AudioQuality::Flac,
                AudioQuality::Mp3_320,
                AudioQuality::Mp3_128
            ]
        );
    }

    #[test]
    fn no_fallback_single_quality() {
        let q = QualityPref::from_mode(QualityMode::Fixed(AudioQuality::Mp3_320), false);
        assert_eq!(q.chain(), vec![AudioQuality::Mp3_320]);
    }

    #[test]
    fn save_load_roundtrip() {
        with_isolated_config(|| {
            let c = Config {
                quality: QualityPref::from_mode(QualityMode::Fixed(AudioQuality::Flac), false),
            };
            c.save().unwrap();
            let loaded = Config::load();
            assert_eq!(loaded.quality.mode, "flac");
            assert!(!loaded.quality.fallback);
            assert_eq!(
                loaded.quality.mode_enum(),
                QualityMode::Fixed(AudioQuality::Flac)
            );
        });
    }

    #[test]
    fn load_missing_returns_default() {
        with_isolated_config(|| {
            let c = Config::load();
            assert_eq!(c, Config::default());
        });
    }

    #[test]
    fn corrupt_config_falls_back_to_default() {
        with_isolated_config(|| {
            let path = Config::path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "not [valid toml {{{").unwrap();
            assert_eq!(Config::load(), Config::default());
        });
    }

    #[test]
    fn alias_roundtrip() {
        for q in [
            AudioQuality::Master,
            AudioQuality::HiRes,
            AudioQuality::Atmos,
            AudioQuality::Flac,
            AudioQuality::Aac,
            AudioQuality::Mp3_320,
            AudioQuality::Mp3_128,
        ] {
            assert_eq!(AudioQuality::from_alias(&q.to_alias()), Some(q));
        }
        assert_eq!(
            AudioQuality::from_alias("320k"),
            Some(AudioQuality::Mp3_320)
        );
        assert_eq!(AudioQuality::from_alias("hires"), Some(AudioQuality::HiRes));
        assert_eq!(AudioQuality::from_alias("bogus"), None);
    }
}
