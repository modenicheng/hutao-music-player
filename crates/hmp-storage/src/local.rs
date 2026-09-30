//! 本地媒体文件：标签元数据读取（lofty）与扩展名过滤。
//!
//! 播放 URI 恒为 `file://<path>`，稳定身份 = 路径本身
//! （`local:<path>`，见 hmp-core `PlayRequest::Local`）。

use std::path::{Path, PathBuf};

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
    /// ReplayGain 曲目增益（dB；无标签 None）。
    pub replaygain_track_db: Option<f64>,
}

/// 解析 ReplayGain 标签文本（`-6.50 dB` / `+3.0 dB` / `12.34dB`；大小写不敏感）。
/// 失败/乱串 → None（不阻断元数据读取）。
pub fn parse_rg_db(s: &str) -> Option<f64> {
    let t = s.trim();
    let t = t
        .strip_suffix("dB")
        .or_else(|| t.strip_suffix("db"))
        .or_else(|| t.strip_suffix("Db"))
        .or_else(|| t.strip_suffix("DB"))
        .unwrap_or(t)
        .trim();
    if t.is_empty() {
        return None;
    }
    let v: f64 = t.parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    Some(v)
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
        replaygain_track_db: tag
            .and_then(|t| t.get_string(&ItemKey::ReplayGainTrackGain))
            .and_then(parse_rg_db),
    })
}

/// 本地歌词候选路径（同目录，按优先级）：`<stem>.lrc`（主流约定，
/// Windows/macOS 大小写不敏感文件系统上天然匹配任意大小写变体）→
/// `<完整文件名>.lrc`（`song.mp3.lrc` 少数工具产物）。
pub fn sidecar_lrc_candidates(audio: &Path) -> Vec<PathBuf> {
    let dir = audio.parent().unwrap_or_else(|| Path::new("."));
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |name: std::ffi::OsString| {
        let candidate = dir.join(name);
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    };
    for name in [
        audio.file_stem().map(|s| {
            let mut n = s.to_os_string();
            n.push(".lrc");
            n
        }),
        audio.file_name().map(|s| {
            let mut n = s.to_os_string();
            n.push(".lrc");
            n
        }),
    ]
    .into_iter()
    .flatten()
    {
        push(name);
    }
    out
}

/// 读取同名 sidecar 歌词（候选序取第一个存在且非空白的文件）。
pub fn read_sidecar_lrc(audio: &Path) -> Option<String> {
    let text = sidecar_lrc_candidates(audio)
        .iter()
        .find_map(|p| std::fs::read(p).ok().filter(|b| !b.is_empty()))?;
    let text = decode_lrc_text(&text);
    (!text.trim().is_empty()).then_some(text)
}

/// 读取内嵌歌词标签（ID3v2 USLT / MP4 ©lyr / APE LYRICS 等，经 lofty
/// `ItemKey::Lyrics` 归一）；无标签或标签为空 → None。
pub fn read_embedded_lyrics(path: &Path) -> Option<String> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let text = tag.get_string(&ItemKey::Lyrics)?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// 歌词文本解码：BOM 识别 UTF-8/UTF-16LE/BE；无 BOM 但含 NUL 字节按
/// UTF-16LE 兜底（部分中文播放器导出）；其余按 UTF-8（非法序列有损替换）。
/// 剥除首部 BOM 残留（`\u{feff}` 非 whitespace，不剥会毒化首行标签解析）。
pub fn decode_lrc_text(bytes: &[u8]) -> String {
    let (payload, utf16le) = if bytes.starts_with(&[0xFF, 0xFE]) {
        (&bytes[2..], Some(true))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        (&bytes[2..], Some(false))
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (&bytes[3..], None)
    } else if bytes.contains(&0) {
        (bytes, Some(true))
    } else {
        (bytes, None)
    };
    let mut text = match utf16le {
        Some(le) => {
            let units: Vec<u16> = bytes_as_u16(payload, le).collect();
            String::from_utf16_lossy(&units)
        }
        None => String::from_utf8_lossy(payload).into_owned(),
    };
    if let Some(stripped) = text.strip_prefix('\u{feff}') {
        text = stripped.to_owned();
    }
    text
}

fn bytes_as_u16(bytes: &[u8], little_endian: bool) -> impl Iterator<Item = u16> + '_ {
    bytes.chunks_exact(2).map(move |pair| {
        if little_endian {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        }
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

    /// G2：ReplayGain 标签项映射与读取（read_meta 的一行读取路径去风险）。
    #[test]
    fn replaygain_tag_item_reads_back() {
        let mut tag = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        tag.insert_text(lofty::tag::ItemKey::ReplayGainTrackGain, "-6.50 dB".into());
        let got = tag.get_string(&lofty::tag::ItemKey::ReplayGainTrackGain);
        assert_eq!(got, Some("-6.50 dB"));
        assert_eq!(got.and_then(parse_rg_db), Some(-6.5));
    }

    /// G2：ReplayGain 标签文本（如 `-6.50 dB`）解析为 dB 值。
    #[test]
    fn parses_replaygain_db() {
        assert_eq!(parse_rg_db("-6.50 dB"), Some(-6.5));
        assert_eq!(parse_rg_db("+3.0 dB"), Some(3.0));
        assert_eq!(parse_rg_db("0 dB"), Some(0.0));
        assert_eq!(parse_rg_db("12.34dB"), Some(12.34));
        assert_eq!(parse_rg_db("-23.83 db"), Some(-23.83));
        assert_eq!(parse_rg_db(""), None);
        assert_eq!(parse_rg_db("abc"), None);
        assert_eq!(parse_rg_db("NaN dB"), None);
    }

    #[test]
    fn read_meta_unreadable_content_returns_none() {
        // 存在但非音频内容 → None（Probe 失败）
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fake.mp3");
        std::fs::write(&p, b"not an audio file at all").unwrap();
        assert!(read_meta(&p).is_none());
    }

    /// sidecar 候选序：`<stem>.lrc` 优先，其次 `<完整文件名>.lrc`；去重。
    #[test]
    fn sidecar_candidates_order_and_dedup() {
        let cands = sidecar_lrc_candidates(Path::new("/music/夜曲.mp3"));
        assert_eq!(
            cands,
            vec![
                PathBuf::from("/music/夜曲.lrc"),
                PathBuf::from("/music/夜曲.mp3.lrc"),
            ]
        );
        // 无扩展名文件：stem == 文件名 → 两候选合并为一个
        let cands = sidecar_lrc_candidates(Path::new("track"));
        assert_eq!(cands, vec![PathBuf::from("track.lrc")]);
    }

    /// sidecar 读取：命中 `<stem>.lrc`；空白文件视为无歌词。
    #[test]
    fn sidecar_lrc_reads_and_skips_blank() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("song.mp3");
        std::fs::write(&audio, b"x").unwrap();
        assert!(read_sidecar_lrc(&audio).is_none());

        std::fs::write(dir.path().join("song.lrc"), "  \r\n").unwrap();
        assert!(read_sidecar_lrc(&audio).is_none());

        std::fs::write(dir.path().join("song.lrc"), "[00:01.00]hello\n").unwrap();
        assert_eq!(
            read_sidecar_lrc(&audio).as_deref(),
            Some("[00:01.00]hello\n")
        );
    }

    /// 歌词解码：UTF-8/UTF-16 BOM 与无 BOM UTF-16 兜底，剥 BOM 残留。
    #[test]
    fn decodes_lrc_encodings() {
        // UTF-8 BOM
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend_from_slice("[00:01]utf8".as_bytes());
        assert_eq!(decode_lrc_text(&bytes), "[00:01]utf8");
        // 无 BOM UTF-8 原样
        assert_eq!(decode_lrc_text(b"[00:01]plain"), "[00:01]plain");
        // UTF-16LE BOM（中文）
        let text = "[00:01]你好";
        let units: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend_from_slice(&units);
        assert_eq!(decode_lrc_text(&bytes), text);
        // 无 BOM UTF-16LE（含 NUL 字节触发兜底）
        let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode_lrc_text(&bytes), text);
        // UTF-16BE BOM
        let units: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend_from_slice(&units);
        assert_eq!(decode_lrc_text(&bytes), text);
    }

    /// 内嵌歌词：ItemKey::Lyrics 标签项映射可读取（USLT/©lyr 归一入口）。
    #[test]
    fn lyrics_tag_item_reads_back() {
        let mut tag = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        tag.insert_text(ItemKey::Lyrics, "[00:01.00]embedded\n".into());
        let got = tag.get_string(&ItemKey::Lyrics);
        assert_eq!(got, Some("[00:01.00]embedded\n"));
    }

    #[test]
    fn embedded_lyrics_missing_file_returns_none() {
        assert!(read_embedded_lyrics(Path::new("/nonexistent/x.mp3")).is_none());
    }
}
