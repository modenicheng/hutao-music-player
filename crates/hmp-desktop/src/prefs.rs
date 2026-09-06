//! UI 偏好持久化（localStorage 对应物）：主题模式 / 音质偏好 / 音量。
//! 存 `~/.config/hmp/desktop-ui.json`（XDG_CONFIG_HOME 优先）。损坏文件按缺省处理。

use std::path::PathBuf;

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    /// 0=跟随系统 1=浅色 2=深色（Theme.mode）
    pub theme_mode: i32,
    /// 0=标准 1=高清 2=无损 3=Hi-Res（Quality.selected）
    pub quality: i32,
    /// 0.0..1.0
    pub volume: f32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme_mode: 0,
            quality: 2,
            volume: 1.0,
        }
    }
}

impl Prefs {
    fn clamp(mut self) -> Self {
        self.theme_mode = self.theme_mode.clamp(0, 2);
        self.quality = self.quality.clamp(0, 3);
        if !self.volume.is_finite() {
            self.volume = 1.0;
        }
        self.volume = self.volume.clamp(0.0, 1.0);
        self
    }
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("hmp").join("desktop-ui.json"))
}

pub fn load() -> Prefs {
    let Some(path) = config_path() else {
        return Prefs::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Prefs::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        tracing::warn!("desktop-ui.json 损坏，按缺省偏好处理");
        return Prefs::default();
    };
    Prefs {
        theme_mode: value["theme_mode"].as_i64().unwrap_or(0) as i32,
        quality: value["quality"].as_i64().unwrap_or(2) as i32,
        volume: value["volume"].as_f64().unwrap_or(1.0) as f32,
    }
    .clamp()
}

pub fn store(prefs: &Prefs) {
    let Some(path) = config_path() else {
        return;
    };
    let value = serde_json::json!({
        "theme_mode": prefs.theme_mode,
        "quality": prefs.quality,
        "volume": prefs.volume,
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(&path, value.to_string()) {
        tracing::warn!("写入偏好失败: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_out_of_range() {
        let prefs = Prefs {
            theme_mode: 9,
            quality: -3,
            volume: f32::NAN,
        }
        .clamp();
        assert_eq!(prefs.theme_mode, 2);
        assert_eq!(prefs.quality, 0);
        assert_eq!(prefs.volume, 1.0);
    }
}
