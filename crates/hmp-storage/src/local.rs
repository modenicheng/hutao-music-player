//! 本地媒体文件：标签元数据读取（lofty）与扩展名过滤。
//!
//! 播放 URI 恒为 `file://<path>`，稳定身份 = 路径本身
//! （`local:<path>`，见 hmp-core `PlayRequest::Local`）。

use std::path::Path;

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey};

/// 支持的音频扩展名。
pub fn is_audio_ext(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("mp3" | "flac" | "ogg" | "m4a" | "opus" | "wav" | "aac" | "ape" | "aiff")
    )
}

/// 本地文件元数据（无标签时为 None，由调用方回退文件名）。
/// 里程碑 E：完整元数据 + 多艺术家 + 内嵌封面。
#[derive(Clone, Debug, Default)]
pub struct LocalMeta {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub format: Option<String>,
    pub bitrate: Option<i64>,
    pub sample_rate: Option<i64>,
    /// 完整艺术家列表（track_artists 写入；空 = 无标签）。
    pub artists: Vec<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u16>,
    pub disc_number: Option<u16>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    /// 内嵌封面原图（前 2MB；无封面 None）。
    pub cover: Option<Vec<u8>>,
}

/// 读取标签元数据；无标签/不可解析 → None。
pub fn read_meta(path: &Path) -> Option<LocalMeta> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged.primary_tag();
    let props = tagged.properties();
    let artists: Vec<String> = tag
        .map(|t| {
            t.get_strings(&ItemKey::TrackArtist)
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    let cover = tag.and_then(|t| {
        t.get_picture_type(PictureType::CoverFront)
            .or_else(|| t.pictures().first())
            .map(|p| p.data())
            .filter(|d| !d.is_empty() && d.len() <= 2 * 1024 * 1024)
            .map(|d| d.to_vec())
    });
    Some(LocalMeta {
        title: tag
            .and_then(|t| t.title())
            .map(|s| s.to_string())
            .unwrap_or_default(),
        artist: tag.and_then(|t| t.artist()).map(|s| s.to_string()),
        album: tag.and_then(|t| t.album()).map(|s| s.to_string()),
        duration_ms: Some(props.duration().as_millis() as i64),
        format: path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase()),
        bitrate: props.audio_bitrate().map(|b| b as i64),
        sample_rate: props.sample_rate().map(|r| r as i64),
        artists,
        album_artist: tag
            .and_then(|t| t.get_string(&ItemKey::AlbumArtist))
            .map(|s| s.to_string()),
        track_number: tag.and_then(|t| t.track()).map(|n| n as u16),
        disc_number: tag.and_then(|t| t.disk()).map(|n| n as u16),
        year: tag.and_then(|t| t.year()).map(|y| y as i64),
        genre: tag.and_then(|t| t.genre()).map(|s| s.to_string()),
        cover,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_filter() {
        assert!(is_audio_ext(Path::new("/a/b.mp3")));
        assert!(is_audio_ext(Path::new("/a/b.FLAC")));
        assert!(is_audio_ext(Path::new("/a/b.ogg")));
        assert!(is_audio_ext(Path::new("/a/b.ape")));
        assert!(is_audio_ext(Path::new("/a/b.aiff")));
        assert!(is_audio_ext(Path::new("/a/b.AIFF")));
        assert!(!is_audio_ext(Path::new("/a/b.txt")));
        assert!(!is_audio_ext(Path::new("/a/b")));
    }

    #[test]
    fn read_meta_missing_file_returns_none() {
        assert!(read_meta(Path::new("/nonexistent/x.mp3")).is_none());
    }

    #[test]
    fn read_meta_unreadable_content_returns_none() {
        // 存在但非音频内容 → None（Probe 失败）
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fake.mp3");
        std::fs::write(&p, b"not an audio file at all").unwrap();
        assert!(read_meta(&p).is_none());
    }
}
