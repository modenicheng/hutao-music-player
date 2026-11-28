//! 本地媒体文件：标签元数据读取（lofty）与扩展名过滤。
//!
//! 播放 URI 恒为 `file://<path>`，稳定身份 = 路径本身
//! （`local:<path>`，见 hmp-core `PlayRequest::Local`）。

use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag};

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
    /// 封面原图（内嵌图优先，超过 32MB 丢弃；无内嵌图时退同目录的封面文件
    /// `folder_cover`；都无 → None）。
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

/// 内嵌封面字节上限：3000-4000px JPEG 封面 3-15MB 常见，旧 2MB 上限把高清
/// 封面整张静默滤成「无封面」（§18）；仍保留上限防病态文件撑爆扫描内存/落盘。
const MAX_COVER_BYTES: usize = 32 * 1024 * 1024;

/// 目录封面主名（小写 + 首字母大写变体）——探测顺序：主名 → 变体 → 扩展名。
const FOLDER_COVER_NAMES: [(&str, &str); 5] = [
    ("cover", "Cover"),
    ("folder", "Folder"),
    ("front", "Front"),
    ("album", "Album"),
    ("albumart", "Albumart"),
];

/// 目录封面扩展名（只认小写；大小写变体按主名的两条拼写）。
const FOLDER_COVER_EXTS: [&str; 3] = ["jpg", "jpeg", "png"];

/// 目录封面候选文件名 `(主名, 扩展名)`，按探测顺序。
/// 读标签探测 [`folder_cover`] 与 watcher 事件识别 [`is_folder_cover_path`]
/// 共用这一份清单——候选集合只在这里定义，不要在调用方复制文件名列表。
fn folder_cover_names() -> impl Iterator<Item = (&'static str, &'static str)> {
    FOLDER_COVER_NAMES.iter().flat_map(|(lower, capitalized)| {
        [*lower, *capitalized]
            .into_iter()
            .flat_map(|stem| FOLDER_COVER_EXTS.iter().map(move |ext| (stem, *ext)))
    })
}

/// 路径是否是目录封面文件（watcher 事件识别；与私有 `folder_cover` 同源）。
///
/// 只按文件名判断（删除事件时文件已不存在）：主名/扩展名必须落在候选集合内，
/// 大小写变体按探测规则——小写主名或首字母大写，扩展名小写。
pub fn is_folder_cover_path(p: &Path) -> bool {
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    FOLDER_COVER_NAMES
        .iter()
        .any(|(lower, capitalized)| stem == *lower || stem == *capitalized)
        && FOLDER_COVER_EXTS.contains(&ext)
}

/// 目录封面探测（无内嵌图时的兜底）：音频所在目录的 `cover` / `folder` /
/// `front` / `album` / `albumart`（各含首字母大写变体）× `jpg` / `jpeg` / `png`，
/// 按「主名 → 大小写变体 → 扩展名」取第一个存在的常规文件并返回其字节。
/// 命中即停，不遍历目录；超过 `MAX_COVER_BYTES` 视为没有（只 stat 不读盘，
/// 病态大文件不进内存）。
fn folder_cover(dir: &Path) -> Option<Vec<u8>> {
    for (stem, ext) in folder_cover_names() {
        let candidate = dir.join(format!("{stem}.{ext}"));
        let Ok(md) = std::fs::metadata(&candidate) else {
            continue;
        };
        if !md.is_file() {
            continue;
        }
        if md.len() > MAX_COVER_BYTES as u64 {
            return None;
        }
        return std::fs::read(&candidate)
            .ok()
            .filter(|b| !b.is_empty() && b.len() <= MAX_COVER_BYTES);
    }
    None
}

/// 解析年份标签文本：取第一段连续 4 位数字（标准日期 `2020-05-01`、
/// `©day` 的 `2020-05-01T07:00:00Z`、紧凑 `20200501` 都落到 2020）；
/// 没有 → None（不阻断元数据读取）。
fn parse_year(s: &str) -> Option<i64> {
    s.as_bytes()
        .windows(4)
        .find(|w| w.iter().all(u8::is_ascii_digit))
        .and_then(|w| std::str::from_utf8(w).ok())
        .and_then(|d| d.parse().ok())
}

/// 标签年份：先读 `RecordingDate`（lofty 0.25 把标准日期标签——Vorbis `DATE` /
/// ID3v2 `TDRC` / MP4 `©day` / RIFF `ICRD`——都映射到它，只有非标准 `YEAR`
/// 才是 `ItemKey::Year`），读不出年份再退 `Year`。
fn tag_year(tag: &Tag) -> Option<i64> {
    [ItemKey::RecordingDate, ItemKey::Year]
        .into_iter()
        .filter_map(|key| tag.get_string(key))
        .find_map(parse_year)
}

/// 读取标签元数据；无标签/不可解析 → None。
///
/// 字段按「primary tag → 其余 tag」逐个兜底，而不是只读 primary：WAV 的
/// RIFF `LIST/INFO` 标签不是 primary（文件里同时有 ID3 时 primary 是 ID3），
/// 旧实现对纯 RIFF INFO 的 WAV 读不到标题/歌手/年份（AUDIT §30.7 遗留）。
pub fn read_meta(path: &Path) -> Option<LocalMeta> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let props = tagged.properties();
    let tags: Vec<&Tag> = tagged
        .primary_tag()
        .into_iter()
        .chain(tagged.tags().iter())
        .collect();
    let artists: Vec<String> = tags
        .iter()
        .find_map(|tag| {
            let values: Vec<String> = tag
                .get_strings(ItemKey::TrackArtist)
                .map(|s| s.to_string())
                .collect();
            (!values.is_empty()).then_some(values)
        })
        .unwrap_or_default();
    // 封面：按同一顺序取第一张（CoverFront → 首图）；全无内嵌图再退同目录封面文件。
    let cover = tags
        .iter()
        .find_map(|tag| {
            tag.get_picture_type(PictureType::CoverFront)
                .or_else(|| tag.pictures().first())
        })
        .map(|picture| picture.data())
        .filter(|data| !data.is_empty() && data.len() <= MAX_COVER_BYTES)
        .map(|data| data.to_vec())
        .or_else(|| folder_cover(path.parent().unwrap_or_else(|| Path::new("."))));
    Some(LocalMeta {
        title: tags
            .iter()
            .find_map(|tag| tag.title())
            .map(|s| s.to_string())
            .unwrap_or_default(),
        artist: tags
            .iter()
            .find_map(|tag| tag.artist())
            .map(|s| s.to_string()),
        album: tags
            .iter()
            .find_map(|tag| tag.album())
            .map(|s| s.to_string()),
        duration_ms: Some(props.duration().as_millis() as i64),
        format: path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase()),
        bitrate: props.audio_bitrate().map(|b| b as i64),
        sample_rate: props.sample_rate().map(|r| r as i64),
        artists,
        album_artist: tags
            .iter()
            .find_map(|tag| tag.get_string(ItemKey::AlbumArtist))
            .map(str::to_string),
        track_number: tags.iter().find_map(|tag| tag.track()).map(|n| n as u16),
        disc_number: tags.iter().find_map(|tag| tag.disk()).map(|n| n as u16),
        year: tags.iter().find_map(|tag| tag_year(tag)),
        genre: tags
            .iter()
            .find_map(|tag| tag.genre())
            .map(|s| s.to_string()),
        cover,
        replaygain_track_db: tags
            .iter()
            .find_map(|tag| tag.get_string(ItemKey::ReplayGainTrackGain))
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

/// 读取内嵌歌词标签（ID3v2 USLT / MP4 ©lyr / APE LYRICS 等）。lofty 0.25 起
/// ID3v2 把 USLT 映射到 `ItemKey::UnsyncLyrics`（`Lyrics` 键在 ID3v2 不受支持），
/// FLAC/Vorbis/MP4 仍走 `Lyrics`——两条键做回退链，两种容器都能读出。
pub fn read_embedded_lyrics(path: &Path) -> Option<String> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let text = tag
        .get_string(ItemKey::UnsyncLyrics)
        .or_else(|| tag.get_string(ItemKey::Lyrics))?;
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
    bytes.as_chunks::<2>().0.iter().map(move |pair| {
        if little_endian {
            u16::from_le_bytes(*pair)
        } else {
            u16::from_be_bytes(*pair)
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
        let got = tag.get_string(lofty::tag::ItemKey::ReplayGainTrackGain);
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

    /// AUDIT §30.7：年份文本取第一段连续 4 位数字，没有 → None。
    #[test]
    fn parses_year_from_date_text() {
        assert_eq!(parse_year("2020"), Some(2020));
        assert_eq!(parse_year("2020-05-01"), Some(2020));
        assert_eq!(parse_year("2020-00-00"), Some(2020));
        assert_eq!(parse_year("2020-05-01T07:00:00Z"), Some(2020), "取第一段");
        assert_eq!(parse_year("20200501"), Some(2020));
        assert_eq!(parse_year("  1999 "), Some(1999));
        assert_eq!(parse_year(""), None);
        assert_eq!(parse_year("   "), None);
        assert_eq!(parse_year("Unknown"), None);
        assert_eq!(parse_year("202"), None, "不足 4 位");
    }

    /// AUDIT §30.7：标准日期标签（Vorbis `DATE`）lofty 0.25 映射为
    /// `RecordingDate`，只有非标准 `YEAR` 才是 `ItemKey::Year`——旧实现只读
    /// Year，FLAC/MP3/M4A 的年份恒空。先读前者，读不出再退后者。
    #[test]
    fn year_reads_recording_date_then_year() {
        let mut tag = lofty::tag::Tag::new(lofty::tag::TagType::VorbisComments);
        // 无日期 → None
        assert_eq!(tag_year(&tag), None);
        // 空日期 → None
        tag.insert_text(ItemKey::RecordingDate, "".into());
        assert_eq!(tag_year(&tag), None);
        // 标准 DATE=2020-05-01：落在 RecordingDate，Year 键读不到
        tag.insert_text(ItemKey::RecordingDate, "2020-05-01".into());
        assert_eq!(tag.get_string(ItemKey::Year), None, "DATE 不映射到 Year");
        assert_eq!(tag_year(&tag), Some(2020));
        // 只有非标准 YEAR=2019：回退读到
        let mut year_only = lofty::tag::Tag::new(lofty::tag::TagType::VorbisComments);
        year_only.insert_text(ItemKey::Year, "2019".into());
        assert_eq!(year_only.get_string(ItemKey::RecordingDate), None);
        assert_eq!(tag_year(&year_only), Some(2019));
        // 两者都有：RecordingDate 优先
        tag.insert_text(ItemKey::Year, "2019".into());
        assert_eq!(tag_year(&tag), Some(2020));
        // DATE 读不出年份（乱串）→ 仍退 Year
        tag.insert_text(ItemKey::RecordingDate, "unknown".into());
        assert_eq!(tag_year(&tag), Some(2019));
    }

    /// 最小合法 PNG（2×2 不透明）；`image` 已是本 crate 依赖。
    fn png_bytes() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([12, 34, 56, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    /// 文件系统是否区分大小写（Windows / macOS 默认不区分）。
    fn fs_is_case_sensitive(dir: &Path) -> bool {
        let probe = dir.join("hmp-case-probe.txt");
        std::fs::write(&probe, b"").unwrap();
        let sensitive = !dir.join("HMP-CASE-PROBE.TXT").exists();
        let _ = std::fs::remove_file(&probe);
        sensitive
    }

    /// AUDIT §30.7：无内嵌图时退同目录封面文件——固定候选名 × 扩展名按序
    /// 探测，命中即停；首字母大写变体同样认；超限视为没有；非常规文件跳过。
    #[test]
    fn folder_cover_probes_fixed_names() {
        let png = png_bytes();
        // 命中：内容按字节原样返回（不按扩展名解码）。
        let hit = tempfile::tempdir().unwrap();
        std::fs::write(hit.path().join("cover.jpg"), &png).unwrap();
        assert_eq!(folder_cover(hit.path()), Some(png.clone()));
        // 无封面目录 → None。
        let bare = tempfile::tempdir().unwrap();
        std::fs::write(bare.path().join("song.flac"), b"x").unwrap();
        assert_eq!(folder_cover(bare.path()), None);
        // 首字母大写变体 + 靠后的主名/扩展名。
        let cap = tempfile::tempdir().unwrap();
        std::fs::write(cap.path().join("Front.jpeg"), &png).unwrap();
        assert_eq!(folder_cover(cap.path()), Some(png.clone()));
        // 优先序：主名 → 大小写变体 → 扩展名。
        let order = tempfile::tempdir().unwrap();
        std::fs::write(order.path().join("front.jpg"), b"front").unwrap();
        std::fs::write(order.path().join("folder.jpg"), b"folder").unwrap();
        assert_eq!(
            folder_cover(order.path()),
            Some(b"folder".to_vec()),
            "主名 folder 先于 front"
        );
        std::fs::write(order.path().join("cover.png"), b"png").unwrap();
        assert_eq!(
            folder_cover(order.path()),
            Some(b"png".to_vec()),
            "主名 cover 先于 folder"
        );
        std::fs::write(order.path().join("cover.jpg"), b"jpg").unwrap();
        assert_eq!(
            folder_cover(order.path()),
            Some(b"jpg".to_vec()),
            "同主名 jpg 先于 png"
        );
        // 大小写变体：Windows NTFS 不区分大小写，`Cover.jpg` 就是 `cover.jpg`
        // 本身（写入即覆盖），两者之间的先后无从观察——按文件系统实际行为断言。
        std::fs::write(order.path().join("Cover.jpg"), b"cap").unwrap();
        if fs_is_case_sensitive(order.path()) {
            assert_eq!(
                folder_cover(order.path()),
                Some(b"jpg".to_vec()),
                "小写主名先于大写变体"
            );
            std::fs::remove_file(order.path().join("cover.jpg")).unwrap();
            assert_eq!(
                folder_cover(order.path()),
                Some(b"png".to_vec()),
                "小写主名的全部扩展名先于大写变体"
            );
        } else {
            assert_eq!(
                folder_cover(order.path()),
                Some(b"cap".to_vec()),
                "不区分大小写的文件系统：Cover.jpg 即 cover.jpg"
            );
        }
        // 非常规文件（同名目录）不算命中，跳到下一个候选。
        let odd = tempfile::tempdir().unwrap();
        std::fs::create_dir(odd.path().join("cover.jpg")).unwrap();
        assert_eq!(folder_cover(odd.path()), None, "目录不算封面");
        std::fs::write(odd.path().join("albumart.jpg"), b"albumart").unwrap();
        assert_eq!(
            folder_cover(odd.path()),
            Some(b"albumart".to_vec()),
            "跳过目录继续找"
        );
        // 超限视为没有（稀疏文件，不实占磁盘）：首个命中即停，不退而取后面的候选。
        let big = tempfile::tempdir().unwrap();
        let f = std::fs::File::create(big.path().join("cover.jpg")).unwrap();
        f.set_len(MAX_COVER_BYTES as u64 + 1).unwrap();
        std::fs::write(big.path().join("front.jpg"), b"front").unwrap();
        assert_eq!(
            folder_cover(big.path()),
            None,
            "超限 → 没有，不退而取 front.jpg"
        );
    }

    /// 目录封面候选单一事实源：watcher 识别（`is_folder_cover_path`）与读标签
    /// 探测（`folder_cover`）必须认同一组文件名；非候选不认。
    #[test]
    fn folder_cover_path_matches_probe_candidates() {
        let png = png_bytes();
        let root = tempfile::tempdir().unwrap();
        for (i, (stem, ext)) in folder_cover_names().enumerate() {
            // 每个候选单独目录：避免优先序遮蔽，逐个确认「识别 == 能读到」。
            let dir = root.path().join(format!("case-{i}"));
            std::fs::create_dir(&dir).unwrap();
            let file = dir.join(format!("{stem}.{ext}"));
            std::fs::write(&file, &png).unwrap();
            assert!(
                is_folder_cover_path(&file),
                "{} 应是封面候选",
                file.display()
            );
            assert_eq!(
                folder_cover(&dir),
                Some(png.clone()),
                "{} 应被探测到",
                file.display()
            );
        }
        // 非候选：扩展名不在集合、主名前缀/后缀不符、不是文件名。
        let odd = root.path().join("odd");
        std::fs::create_dir(&odd).unwrap();
        for name in [
            "cover.gif",
            "cover.webp",
            "mycover.jpg",
            "cover.tar.jpg",
            "front.bmp",
        ] {
            let f = odd.join(name);
            std::fs::write(&f, &png).unwrap();
            assert!(
                !is_folder_cover_path(&f),
                "{name} 不在候选集合内（探测同样不认）"
            );
        }
        assert_eq!(folder_cover(&odd), None, "非候选不应被探测到");
        // 全大写扩展名：探测在不区分大小写的文件系统上会经 `cover.jpg` 命中
        // 同一个文件（§30.7），事件识别只认候选拼写（扩展名小写）——两者可
        // 观察的差异只在 Windows，这里只钉死识别侧，不假定文件系统语义。
        let upper_ext = odd.join("COVER.JPG");
        std::fs::write(&upper_ext, &png).unwrap();
        assert!(
            !is_folder_cover_path(&upper_ext),
            "扩展名大小写变体不是候选"
        );
        assert!(!is_folder_cover_path(&odd), "目录本身不是封面文件");
    }

    /// RIFF 块：FourCC + LE 长度 + 内容；奇数长度补 1 字节对齐（不计入长度）。
    fn push_chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(fourcc);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
    }

    /// 手造最小合法 WAV（8kHz 单声道 16bit PCM 静音，参数同 hmp-player 测试），
    /// 可带 `ID3 ` 块：ID3v2.4 UTF-8 文本帧（`TIT2`/`TPE1`/`TDRC` = lofty 的
    /// 标准标题/歌手/RecordingDate 键）与内嵌封面帧（APIC）。
    fn minimal_wav(frames: &[(&[u8; 4], &str)], embedded_png: Option<&[u8]>) -> Vec<u8> {
        minimal_wav_with(frames, embedded_png, &[])
    }

    /// 同 [`minimal_wav`]，另带 RIFF `LIST/INFO` 标签块（无 ID3 的 WAV 场景）。
    fn minimal_wav_with(
        frames: &[(&[u8; 4], &str)],
        embedded_png: Option<&[u8]>,
        info: &[(&[u8; 4], &str)],
    ) -> Vec<u8> {
        let mut fmt = Vec::new();
        for n in [1u16, 1] {
            fmt.extend_from_slice(&n.to_le_bytes()); // PCM / 单声道
        }
        for n in [8000u32, 16000] {
            fmt.extend_from_slice(&n.to_le_bytes()); // 采样率 / 字节率
        }
        for n in [2u16, 16] {
            fmt.extend_from_slice(&n.to_le_bytes()); // 块对齐 / 位深
        }
        // ID3v2.4 帧：标识 + 同步安全长度 + 标志 + 正文。
        fn syncsafe(n: usize) -> [u8; 4] {
            [21, 14, 7, 0].map(|shift| ((n >> shift) & 0x7f) as u8)
        }
        let mut tag = Vec::new();
        let mut push_frame = |id: &[u8; 4], body: &[u8]| {
            tag.extend_from_slice(id);
            tag.extend_from_slice(&syncsafe(body.len()));
            tag.extend_from_slice(&[0, 0]);
            tag.extend_from_slice(body);
        };
        for (id, text) in frames {
            let mut body = vec![3]; // UTF-8
            body.extend_from_slice(text.as_bytes());
            push_frame(id, &body);
        }
        if let Some(png) = embedded_png {
            let mut body = vec![0]; // Latin-1
            body.extend_from_slice(b"image/png\0");
            body.extend_from_slice(&[3, 0]); // PictureType::CoverFront / 空描述
            body.extend_from_slice(png);
            push_frame(b"APIC", &body);
        }
        let mut riff = Vec::new();
        push_chunk(&mut riff, b"fmt ", &fmt);
        push_chunk(&mut riff, b"data", &vec![0u8; 1600]);
        if !tag.is_empty() {
            let mut id3 = b"ID3\x04\x00\x00".to_vec();
            id3.extend_from_slice(&syncsafe(tag.len()));
            id3.extend_from_slice(&tag);
            push_chunk(&mut riff, b"ID3 ", &id3);
        }
        if !info.is_empty() {
            let mut list = b"INFO".to_vec();
            for (id, text) in info {
                let mut body = text.as_bytes().to_vec();
                body.push(0); // RIFF INFO 字符串以 NUL 结尾
                push_chunk(&mut list, id, &body);
            }
            push_chunk(&mut riff, b"LIST", &list);
        }
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&(riff.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(&riff);
        wav
    }

    /// AUDIT §30.7 端到端：真实容器里的标准日期标签（ID3v2.4 `TDRC`）经 lofty
    /// 映射到 RecordingDate——旧实现只读 ItemKey::Year，年份恒空。
    #[test]
    fn read_meta_reads_standard_date_tag() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("a.wav");
        std::fs::write(
            &wav,
            minimal_wav(
                &[
                    (b"TIT2", "夜曲"),
                    (b"TPE1", "周杰伦"),
                    (b"TDRC", "2020-05-01"),
                ],
                None,
            ),
        )
        .unwrap();
        let meta = read_meta(&wav).unwrap();
        assert_eq!(meta.year, Some(2020));
        assert_eq!(meta.title, "夜曲");
        assert_eq!(meta.artist.as_deref(), Some("周杰伦"));
        // 无日期标签 → None
        let plain = dir.path().join("b.wav");
        std::fs::write(&plain, minimal_wav(&[], None)).unwrap();
        assert_eq!(read_meta(&plain).unwrap().year, None);
    }

    /// 纯 RIFF `LIST/INFO` 标签（无 ID3）的 WAV：primary tag 为空/非它，
    /// 字段必须按 tag 顺序兜底读到（AUDIT §30.7 遗留）。
    #[test]
    fn read_meta_reads_riff_info_without_id3() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("info.wav");
        std::fs::write(
            &wav,
            minimal_wav_with(
                &[],
                None,
                &[
                    (b"INAM", "夜曲"),
                    (b"IART", "周杰伦"),
                    (b"IPRD", "十一月的萧邦"),
                    (b"ICRD", "2005"),
                ],
            ),
        )
        .unwrap();
        let meta = read_meta(&wav).unwrap();
        assert_eq!(meta.title, "夜曲");
        assert_eq!(meta.artist.as_deref(), Some("周杰伦"));
        assert_eq!(meta.album.as_deref(), Some("十一月的萧邦"));
        assert_eq!(meta.year, Some(2005));
    }

    /// AUDIT §30.7 端到端：无内嵌图的音频走同目录封面文件（与内嵌图同一
    /// 返回通道 `LocalMeta.cover`）；目录无封面 → None；有内嵌图则内嵌优先。
    #[test]
    fn read_meta_falls_back_to_folder_cover() {
        let png = png_bytes();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cover.jpg"), &png).unwrap();
        let wav = dir.path().join("song.wav");
        std::fs::write(&wav, minimal_wav(&[], None)).unwrap();
        assert_eq!(read_meta(&wav).unwrap().cover, Some(png.clone()));

        let bare = tempfile::tempdir().unwrap();
        let wav = bare.path().join("song.wav");
        std::fs::write(&wav, minimal_wav(&[], None)).unwrap();
        assert_eq!(read_meta(&wav).unwrap().cover, None);

        // 同目录仍有 cover.jpg：内嵌图（APIC）优先。
        let embedded = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        embedded
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let inner = buf.into_inner();
        assert_ne!(inner, png, "两张图字节不同，才能区分来源");
        let wav = dir.path().join("embedded.wav");
        std::fs::write(&wav, minimal_wav(&[], Some(&inner))).unwrap();
        assert_eq!(read_meta(&wav).unwrap().cover, Some(inner), "内嵌图优先");
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
    /// 内嵌歌词：lofty 0.25 ID3v2 把 USLT 映射到 UnsyncLyrics（主路径），
    /// Lyrics 键回退链覆盖 FLAC/Vorbis/MP4——与 read_embedded_lyrics 同构。
    #[test]
    fn lyrics_tag_item_reads_back() {
        let mut tag = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        tag.insert_text(
            ItemKey::UnsyncLyrics,
            "[00:01.00]embedded
"
            .into(),
        );
        assert_eq!(
            tag.get_string(ItemKey::UnsyncLyrics),
            Some(
                "[00:01.00]embedded
"
            )
        );

        let mut vorbis = lofty::tag::Tag::new(lofty::tag::TagType::VorbisComments);
        vorbis.insert_text(
            ItemKey::Lyrics,
            "[00:02.00]flac
"
            .into(),
        );
        assert_eq!(
            vorbis.get_string(ItemKey::Lyrics),
            Some(
                "[00:02.00]flac
"
            )
        );
        // 回退链形态：ID3v2 标签上 Lyrics 键缺位时读 UnsyncLyrics
        let text = tag
            .get_string(ItemKey::UnsyncLyrics)
            .or_else(|| tag.get_string(ItemKey::Lyrics));
        assert_eq!(
            text,
            Some(
                "[00:01.00]embedded
"
            )
        );
    }

    #[test]
    fn embedded_lyrics_missing_file_returns_none() {
        assert!(read_embedded_lyrics(Path::new("/nonexistent/x.mp3")).is_none());
    }
}
