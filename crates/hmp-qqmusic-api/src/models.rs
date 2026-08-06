//! 领域模型（对应上游 `models/base.py` 的共享业务实体）。
//!
//! 字段与上游 pydantic 模型对齐（含 alias 兼容多接口命名差异）；
//! 反序列化使用 serde `#[serde(default)]` 容忍缺省字段。

use serde::Deserialize;

/// 歌手摘要（上游 `Singer`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Singer {
    /// 歌手数字 ID。
    #[serde(default, alias = "singerID", alias = "singerId", alias = "SingerID")]
    pub id: i64,
    /// 歌手 Media MID。
    #[serde(default, alias = "singerMid", alias = "singerMID", alias = "SingerMid")]
    pub mid: String,
    /// 歌手名称。
    #[serde(default, alias = "singerName")]
    pub name: String,
    /// 歌手展示标题。
    #[serde(default)]
    pub title: String,
    /// 歌手类型（0=艺人，1=组合）。
    #[serde(default, alias = "SingerType", alias = "vt")]
    pub type_: i64,
    /// 关联用户 ID。
    #[serde(default)]
    pub uin: i64,
    /// 图片 Media ID。
    #[serde(default, alias = "singerPmid", alias = "pic_mid")]
    pub pmid: String,
}

/// 专辑摘要（上游 `Album`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Album {
    /// 专辑数字 ID。
    #[serde(default, alias = "albumID")]
    pub id: i64,
    /// 专辑 Media MID。
    #[serde(default, alias = "albumMid", alias = "albumMID", alias = "albummid")]
    pub mid: String,
    /// 专辑名称。
    #[serde(default, alias = "albumName")]
    pub name: String,
    /// 专辑展示标题。
    #[serde(default)]
    pub title: String,
    /// 专辑副标题。
    #[serde(default, alias = "albumTranName")]
    pub subtitle: String,
    /// 发行日期（YYYY-MM-DD）。
    #[serde(default, alias = "publish_date", alias = "publishDate")]
    pub time_public: String,
    /// 图片 Media ID。
    #[serde(default, alias = "logo")]
    pub pmid: String,
}

/// 歌曲文件信息（上游 `File`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct File {
    /// 基础媒体标识符。
    #[serde(default)]
    pub media_mid: String,
    /// 极低品质 AAC。
    #[serde(default)]
    pub size_24aac: i64,
    /// 低品质 AAC。
    #[serde(default)]
    pub size_48aac: i64,
    /// 流畅音质 AAC。
    #[serde(default)]
    pub size_96aac: i64,
    /// HQ 高品质 OGG 192k。
    #[serde(default)]
    pub size_192ogg: i64,
    /// HQ 高品质 AAC 192k。
    #[serde(default)]
    pub size_192aac: i64,
    /// 标准音质 MP3 128k。
    #[serde(default)]
    pub size_128mp3: i64,
    /// HQ 高品质 MP3 320k。
    #[serde(default)]
    pub size_320mp3: i64,
    /// SQ 无损 FLAC。
    #[serde(default)]
    pub size_flac: i64,
    /// DTS:X 音效。
    #[serde(default)]
    pub size_dts: i64,
    /// 试听片段。
    #[serde(default)]
    pub size_try: i64,
    /// 试听开始时间（毫秒）。
    #[serde(default)]
    pub try_begin: i64,
    /// 试听结束时间（毫秒）。
    #[serde(default)]
    pub try_end: i64,
    /// 流畅音质 OGG 96k。
    #[serde(default)]
    pub size_96ogg: i64,
    /// 杜比全景声。
    #[serde(default)]
    pub size_dolby: i64,
    /// 现代高级音质数组（臻品系列等）。
    #[serde(default)]
    pub size_new: Vec<i64>,
}

/// 支付属性（上游 `Pay`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Pay {
    /// 绿钻/付费包权限。
    #[serde(default)]
    pub pay_month: i64,
    /// 单曲售价（分）。
    #[serde(default)]
    pub price_track: i64,
    /// 专辑售价（分）。
    #[serde(default)]
    pub price_album: i64,
    /// 播放付费标识。
    #[serde(default)]
    pub pay_play: i64,
    /// 下载付费标识。
    #[serde(default)]
    pub pay_down: i64,
    /// 支付状态。
    #[serde(default)]
    pub pay_status: i64,
    /// 免费试听时长。
    #[serde(default)]
    pub time_free: i64,
}

/// MV 摘要（上游 `MV`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct MV {
    /// MV 数字 ID。
    #[serde(default, alias = "sid", alias = "mvid", alias = "singerId")]
    pub id: i64,
    /// MV VID。
    #[serde(default)]
    pub vid: String,
    /// MV 类型。
    #[serde(default, alias = "vt")]
    pub type_: i64,
    /// MV 名称。
    #[serde(default, alias = "mvname")]
    pub name: String,
    /// MV 展示标题。
    #[serde(default, alias = "title_main")]
    pub title: String,
}

/// 歌曲基础模型（上游 `Song`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Song {
    /// 歌曲数字 ID。
    #[serde(default)]
    pub id: i64,
    /// 歌曲 Media MID（请求播放/歌词/详情的核心参数）。
    #[serde(default)]
    pub mid: String,
    /// 歌曲名称。
    #[serde(default)]
    pub name: String,
    /// 歌曲类型（1=普通歌曲，2=长音频，6=视频/直播）。
    #[serde(default)]
    pub type_: i64,
    /// 歌曲标题。
    #[serde(default)]
    pub title: String,
    /// 副标题。
    #[serde(default)]
    pub subtitle: String,
    /// 歌手列表。
    #[serde(default)]
    pub singer: Vec<Singer>,
    /// 专辑信息。
    #[serde(default)]
    pub album: Album,
    /// MV 信息。
    #[serde(default)]
    pub mv: MV,
    /// 文件信息。
    #[serde(default)]
    pub file: File,
    /// 支付属性。
    #[serde(default)]
    pub pay: Pay,
    /// 时长（秒）。
    #[serde(default)]
    pub interval: i64,
    /// 是否独家（1=是）。
    #[serde(default)]
    pub isonly: i64,
    /// 语言 ID。
    #[serde(default)]
    pub language: i64,
    /// 音乐流派 ID。
    #[serde(default)]
    pub genre: i64,
    /// CD 索引。
    #[serde(default)]
    pub index_cd: i64,
    /// 专辑索引。
    #[serde(default)]
    pub index_album: i64,
    /// 发行日期（YYYY-MM-DD）。
    #[serde(default)]
    pub time_public: String,
    /// 上下架状态（0=正常）。
    #[serde(default)]
    pub status: i64,
    /// 唱片公司/特性标签。
    #[serde(default)]
    pub label: String,
    /// BPM。
    #[serde(default)]
    pub bpm: i64,
    /// 原版标识（1=正宗原版）。
    #[serde(default)]
    pub ov: i64,
    /// 64 位权益位掩码。
    #[serde(default)]
    pub sa: i64,
    /// 扩展状态/来源。
    #[serde(default)]
    pub es: String,
    /// 关联版本与高级媒体 MID 列表。
    #[serde(default)]
    pub vs: Vec<String>,
    /// 变体信息数组。
    #[serde(default)]
    pub vi: Vec<i64>,
    /// 音量平衡数组（ReplayGain）。
    #[serde(default)]
    pub vf: Vec<f64>,
}

impl Song {
    /// 是否具备播放所需的最小字段（mid + name）。
    pub fn has_playable_identity(&self) -> bool {
        !self.mid.is_empty() && !self.name.is_empty()
    }
}
