//! 媒体领域模型（docs/PROJECT.md §7）。
//!
//! 描述曲目/专辑/歌手/歌单的**领域形态**——不含 QQ 接口原始字段
//! （原始响应转换由 `hmp-qqmusic-api` 的适配层负责）。

use serde::{Deserialize, Serialize};

use crate::id::{AlbumId, ArtistId, PlaylistId, TrackId};

/// 歌手引用（嵌入 `Track` 的轻量视图）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    /// 歌手标识符。
    pub id: ArtistId,
    /// 展示名称。
    pub name: String,
}

/// 专辑引用（嵌入 `Track` 的轻量视图）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumRef {
    /// 专辑标识符。
    pub id: AlbumId,
    /// 专辑名称。
    pub name: String,
}

/// 封面引用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverRef {
    /// 封面图片地址（http/https 或本地文件 URI）。
    pub url: String,
}

/// 音频质量（docs/PROJECT.md §7.3）。
///
/// 序列化为字符串；反序列化时已知名映射到对应变体，未知字符串归入
/// [`AudioQuality::Unknown`]（自定义 serde impl）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioQuality {
    /// MP3 128k（标准）。
    Mp3_128,
    /// MP3 320k（HQ）。
    Mp3_320,
    /// AAC。
    Aac,
    /// FLAC 无损。
    Flac,
    /// Hi-Res。
    HiRes,
    /// 全景声。
    Atmos,
    /// 臻品母带。
    Master,
    /// 未知/其他。
    Unknown(String),
}

impl serde::Serialize for AudioQuality {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AudioQuality::Mp3_128 => serializer.serialize_str("Mp3_128"),
            AudioQuality::Mp3_320 => serializer.serialize_str("Mp3_320"),
            AudioQuality::Aac => serializer.serialize_str("Aac"),
            AudioQuality::Flac => serializer.serialize_str("Flac"),
            AudioQuality::HiRes => serializer.serialize_str("HiRes"),
            AudioQuality::Atmos => serializer.serialize_str("Atmos"),
            AudioQuality::Master => serializer.serialize_str("Master"),
            AudioQuality::Unknown(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> serde::Deserialize<'de> for AudioQuality {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let s = String::deserialize(d).map_err(D::Error::custom)?;
        Ok(match s.as_str() {
            "Mp3_128" => AudioQuality::Mp3_128,
            "Mp3_320" => AudioQuality::Mp3_320,
            "Aac" => AudioQuality::Aac,
            "Flac" => AudioQuality::Flac,
            "HiRes" => AudioQuality::HiRes,
            "Atmos" => AudioQuality::Atmos,
            "Master" => AudioQuality::Master,
            other => AudioQuality::Unknown(other.to_owned()),
        })
    }
}

impl AudioQuality {
    /// 按质量从高到低排列（用于可用性检测与展示排序）。
    pub fn ordered() -> [AudioQuality; 6] {
        [
            AudioQuality::Master,
            AudioQuality::HiRes,
            AudioQuality::Atmos,
            AudioQuality::Flac,
            AudioQuality::Mp3_320,
            AudioQuality::Mp3_128,
        ]
    }

    /// 解析 CLI/配置别名（`auto`/`master`/`hires`/`atmos`/`flac`/`aac`/`320`/`128`）。
    pub fn from_alias(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "master" => Some(Self::Master),
            "hires" | "hi-res" => Some(Self::HiRes),
            "atmos" => Some(Self::Atmos),
            "flac" | "lossless" => Some(Self::Flac),
            "aac" => Some(Self::Aac),
            "320" | "320k" | "mp3_320" => Some(Self::Mp3_320),
            "128" | "128k" | "mp3_128" => Some(Self::Mp3_128),
            _ => None,
        }
    }

    /// 别名（`from_alias` 的逆）。
    pub fn to_alias(&self) -> String {
        match self {
            Self::Master => "master".into(),
            Self::HiRes => "hires".into(),
            Self::Atmos => "atmos".into(),
            Self::Flac => "flac".into(),
            Self::Aac => "aac".into(),
            Self::Mp3_320 => "320".into(),
            Self::Mp3_128 => "128".into(),
            Self::Unknown(s) => s.clone(),
        }
    }

    /// 质量回退链（docs/PROJECT.md §7.3）：请求目标音质不可用时逐级降级。
    /// 文档化回退链（docs/PROJECT.md §7.3，含 Atmos）：
    /// `Master` → `HiRes` → `Atmos` → `Flac` → `Mp3_320` → `Mp3_128`。
    ///
    /// 例如 `HiRes` → `Flac` → `Mp3_320` → `Mp3_128`。
    pub fn fallback_chain(self) -> Vec<AudioQuality> {
        match self {
            AudioQuality::Master => vec![
                AudioQuality::Master,
                AudioQuality::HiRes,
                AudioQuality::Atmos,
                AudioQuality::Flac,
                AudioQuality::Mp3_320,
                AudioQuality::Mp3_128,
            ],
            AudioQuality::HiRes => vec![
                AudioQuality::HiRes,
                AudioQuality::Flac,
                AudioQuality::Mp3_320,
                AudioQuality::Mp3_128,
            ],
            AudioQuality::Atmos => vec![
                AudioQuality::Atmos,
                AudioQuality::Flac,
                AudioQuality::Mp3_320,
                AudioQuality::Mp3_128,
            ],
            AudioQuality::Flac => vec![
                AudioQuality::Flac,
                AudioQuality::Mp3_320,
                AudioQuality::Mp3_128,
            ],
            AudioQuality::Aac => vec![AudioQuality::Aac, AudioQuality::Mp3_128],
            AudioQuality::Mp3_320 => vec![AudioQuality::Mp3_320, AudioQuality::Mp3_128],
            AudioQuality::Mp3_128 => vec![AudioQuality::Mp3_128],
            AudioQuality::Unknown(_) => vec![AudioQuality::Mp3_128],
        }
    }
}

/// 源解析中间产物：稳定 ID + 轻量元数据（列表解析附带返回，供媒体库缓存）。
///
/// 播放队列只持有 [`TrackId`]；解析器拿到列表时把 stub 批量缓存进媒体库，
/// 查询投影层（`hmp queue list` 等）再经 SQLite 一次查询映射出标题/歌手——
/// 不在 IPC 里搬运完整 rich metadata，也不在列表查询时逐曲发 song detail。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackStub {
    /// 稳定 ID（QQ mid / `local:<path>`）。
    pub id: TrackId,
    /// 标题（未知时回退为 id 字符串）。
    pub title: String,
    /// 歌手列表。
    pub artists: Vec<String>,
    /// 专辑名（可选）。
    pub album: Option<String>,
    /// 时长毫秒（可选）。
    pub duration_ms: Option<i64>,
}

/// 曲目（docs/PROJECT.md §7.1）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// 歌曲标识符。
    pub id: TrackId,
    /// 歌曲标题。
    pub title: String,
    /// 歌手列表。
    pub artists: Vec<ArtistRef>,
    /// 专辑（可选）。
    pub album: Option<AlbumRef>,
    /// 时长。
    pub duration: Option<std::time::Duration>,
    /// 封面。
    pub cover: Option<CoverRef>,
    /// 当前可播放 URL（取流成功后填充，供 MPRIS `xesam:url`）。
    pub url: Option<String>,
    /// 可用音质（从高到低；探测自 QQ size 字段 + 本次解析成功档位）。
    #[serde(alias = "qualities")]
    pub available_qualities: Vec<AudioQuality>,
}

impl Track {
    /// 构造新曲目。
    pub fn new(id: TrackId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            artists: Vec::new(),
            album: None,
            duration: None,
            cover: None,
            url: None,
            available_qualities: Vec::new(),
        }
    }

    /// 歌手展示名（`"A / B"` 形式，空列表返回空串）。
    pub fn artist_names(&self) -> String {
        self.artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// 是否具备最低播放身份（有 ID 且有标题）。
    pub fn is_playable(&self) -> bool {
        !self.id.0.is_empty() && !self.title.is_empty()
    }
}

/// 专辑（完整形态）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Album {
    /// 专辑标识符。
    pub id: AlbumId,
    /// 专辑名称。
    pub name: String,
    /// 发行日期（YYYY-MM-DD，可空）。
    pub release_date: Option<String>,
    /// 封面。
    pub cover: Option<CoverRef>,
    /// 歌手列表。
    pub artists: Vec<ArtistRef>,
}

/// 歌单。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    /// 歌单标识符。
    pub id: PlaylistId,
    /// 歌单标题。
    pub title: String,
    /// 描述。
    pub description: String,
    /// 封面。
    pub cover: Option<CoverRef>,
    /// 曲目列表（加载后填充）。
    pub tracks: Vec<Track>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_track() -> Track {
        Track::new(TrackId::new("mid-1"), "开始懂了").with_artist("孙燕姿")
    }

    trait TestExt {
        fn with_artist(self, name: &str) -> Self;
    }
    impl TestExt for Track {
        fn with_artist(mut self, name: &str) -> Self {
            self.artists.push(ArtistRef {
                id: ArtistId::new("artist-1"),
                name: name.to_owned(),
            });
            self
        }
    }

    #[test]
    fn track_roundtrips_through_json() {
        let t = sample_track();
        let json = serde_json::to_string(&t).unwrap();
        let back: Track = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn track_carries_playable_url() {
        let mut t = sample_track();
        assert!(t.url.is_none());
        t.url = Some("https://example.com/stream.mp3".into());
        let back: Track = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back.url.as_deref(), Some("https://example.com/stream.mp3"));
    }

    #[test]
    fn quality_fallback_chain_degrades_stepwise() {
        let chain = AudioQuality::HiRes.fallback_chain();
        assert_eq!(
            chain,
            vec![
                AudioQuality::HiRes,
                AudioQuality::Flac,
                AudioQuality::Mp3_320,
                AudioQuality::Mp3_128,
            ]
        );
        assert_eq!(
            AudioQuality::Master.fallback_chain(),
            vec![
                AudioQuality::Master,
                AudioQuality::HiRes,
                AudioQuality::Atmos,
                AudioQuality::Flac,
                AudioQuality::Mp3_320,
                AudioQuality::Mp3_128,
            ]
        );
        assert_eq!(
            AudioQuality::Mp3_128.fallback_chain(),
            vec![AudioQuality::Mp3_128]
        );
    }

    #[test]
    fn quality_ordered_is_descending() {
        let ordered = AudioQuality::ordered();
        assert_eq!(ordered[0], AudioQuality::Master);
        assert_eq!(ordered[5], AudioQuality::Mp3_128);
    }

    #[test]
    fn track_helpers() {
        let t = sample_track();
        assert_eq!(t.artist_names(), "孙燕姿");
        assert!(t.is_playable());
        let empty = Track::new(TrackId::new(""), "");
        assert!(!empty.is_playable());
    }

    #[test]
    fn audio_quality_serializes_with_tag() {
        let v = json!(AudioQuality::Flac);
        assert_eq!(v, "Flac");
        let back: AudioQuality = serde_json::from_value(v).unwrap();
        assert_eq!(back, AudioQuality::Flac);
        // Unknown 携带原文
        let u: AudioQuality = serde_json::from_value(json!("HiRes96")).unwrap();
        assert!(matches!(u, AudioQuality::Unknown(_)));
    }
}
