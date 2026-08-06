//! 歌手模块（对应上游 `modules/singer.py`）。
//!
//! `get_info` / `get_tab_detail` 固定 Android 平台（ct=11/cv=14090008），
//! 通过请求级 comm 覆盖实现；其余接口用默认 Web 平台。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::error::QqMusicError;
use crate::models::{Album, Song};
use crate::protocol::cgi::CgiRequest;

/// Android 平台 comm 覆盖（上游 `VersionProfile.android`）。
fn android_comm() -> Value {
    json!({"ct": 11, "cv": 14090008, "v": 14090008, "platform": "yqq.json"})
}

// ---------------------------------------------------------------------------
// 枚举（上游 singer.py 同名枚举）
// ---------------------------------------------------------------------------

/// 地区类型枚举（上游 `AreaType`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaType {
    /// 全部。
    All,
    /// 内地。
    China,
    /// 台湾。
    Taiwan,
    /// 欧美。
    America,
    /// 日本。
    Japan,
    /// 韩国。
    Korea,
}

impl AreaType {
    /// 对应的数字值。
    pub fn value(self) -> i64 {
        match self {
            AreaType::All => -100,
            AreaType::China => 200,
            AreaType::Taiwan => 2,
            AreaType::America => 5,
            AreaType::Japan => 4,
            AreaType::Korea => 3,
        }
    }
}

/// 风格类型枚举（上游 `GenreType`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenreType {
    /// 全部。
    All,
    /// 流行。
    Pop,
    /// 说唱。
    Rap,
    /// 国风。
    ChineseStyle,
    /// 摇滚。
    Rock,
    /// 电子。
    Electronic,
    /// 民谣。
    Folk,
    /// R&B。
    RAndB,
    /// 民族。
    Ethnic,
    /// 轻音乐。
    LightMusic,
    /// 爵士。
    Jazz,
    /// 古典。
    Classical,
    /// 乡村。
    Country,
    /// 蓝调。
    Blues,
}

impl GenreType {
    /// 对应的数字值。
    pub fn value(self) -> i64 {
        match self {
            GenreType::All => -100,
            GenreType::Pop => 7,
            GenreType::Rap => 3,
            GenreType::ChineseStyle => 19,
            GenreType::Rock => 4,
            GenreType::Electronic => 2,
            GenreType::Folk => 8,
            GenreType::RAndB => 11,
            GenreType::Ethnic => 37,
            GenreType::LightMusic => 93,
            GenreType::Jazz => 14,
            GenreType::Classical => 33,
            GenreType::Country => 13,
            GenreType::Blues => 10,
        }
    }
}

/// 性别类型枚举（上游 `SexType`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SexType {
    /// 全部。
    All,
    /// 男。
    Male,
    /// 女。
    Female,
    /// 组合。
    Group,
}

impl SexType {
    /// 对应的数字值。
    pub fn value(self) -> i64 {
        match self {
            SexType::All => -100,
            SexType::Male => 0,
            SexType::Female => 1,
            SexType::Group => 2,
        }
    }
}

/// 首字母索引枚举（上游 `IndexType`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexType {
    /// 全部。
    All,
    /// 字母 A-Z。
    Letter(u8),
    /// 特殊字符/数字。
    Hash,
}

impl IndexType {
    /// 对应的数字值。
    pub fn value(self) -> i64 {
        match self {
            IndexType::All => -100,
            IndexType::Letter(c) => (c - b'A' + 1) as i64,
            IndexType::Hash => 27,
        }
    }
}

/// 歌手主页 Tab 类型（上游 `TabType`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabType {
    /// 百科。
    Wiki,
    /// 专辑。
    Album,
    /// 作曲。
    Composer,
    /// 作词。
    Lyricist,
    /// 制作人。
    Producer,
    /// 编曲。
    Arranger,
    /// 乐手。
    Musician,
    /// 歌曲。
    Song,
    /// 视频。
    Video,
}

impl TabType {
    /// Tab 标识符。
    pub fn tab_id(self) -> &'static str {
        match self {
            TabType::Wiki => "wiki",
            TabType::Album => "album",
            TabType::Composer => "song_composing",
            TabType::Lyricist => "song_lyric",
            TabType::Producer => "producer",
            TabType::Arranger => "arranger",
            TabType::Musician => "musician",
            TabType::Song => "song_sing",
            TabType::Video => "video",
        }
    }
}

// ---------------------------------------------------------------------------
// 模型（上游 models/singer.py）
// ---------------------------------------------------------------------------

/// 歌手筛选标签项（上游 `TagOption`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TagOption {
    /// 标签 ID。
    #[serde(default)]
    pub id: i64,
    /// 标签名称。
    #[serde(default)]
    pub name: String,
}

/// 歌手列表条目（上游 `SingerBrief`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerBrief {
    /// 歌手 ID。
    #[serde(default, alias = "singerId", alias = "singer_id")]
    pub id: i64,
    /// 歌手 MID。
    #[serde(default, alias = "singerMid", alias = "singer_mid")]
    pub mid: String,
    /// 歌手名称。
    #[serde(default, alias = "singerName", alias = "singer_name")]
    pub name: String,
    /// 图片标识。
    #[serde(default, alias = "singerPmid", alias = "singer_pmid")]
    pub pmid: String,
    /// 地区 ID。
    #[serde(default)]
    pub area_id: i64,
    /// 国家或地区 ID。
    #[serde(default)]
    pub country_id: i64,
    /// 国家或地区名称。
    #[serde(default)]
    pub country: String,
    /// 别名。
    #[serde(default)]
    pub other_name: String,
    /// 拼音。
    #[serde(default)]
    pub spell: String,
    /// 趋势标记。
    #[serde(default)]
    pub trend: i64,
    /// 关注数。
    #[serde(default, alias = "concernNum")]
    pub concern_num: i64,
    /// 歌手图片地址。
    #[serde(default)]
    pub singer_pic: String,
}

/// 歌手筛选标签集合（上游 `SingerTagData`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerTagData {
    /// 地区标签。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub area: Vec<TagOption>,
    /// 流派标签。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub genre: Vec<TagOption>,
    /// 性别标签。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub sex: Vec<TagOption>,
    /// 索引标签。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub index: Vec<TagOption>,
}

/// 歌手列表响应（上游 `SingerTypeListResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerTypeListResponse {
    /// 当前地区筛选值。
    #[serde(default)]
    pub area: i64,
    /// 当前性别筛选值。
    #[serde(default)]
    pub sex: i64,
    /// 当前流派筛选值。
    #[serde(default)]
    pub genre: i64,
    /// 当前返回的歌手列表。
    #[serde(default)]
    pub singerlist: Vec<SingerBrief>,
    /// 返回码。
    #[serde(default)]
    pub code: i64,
    /// 热门歌手列表。
    #[serde(default)]
    pub hotlist: Vec<SingerBrief>,
    /// 可选筛选标签。
    #[serde(default)]
    pub tags: SingerTagData,
}

/// 按索引分页的歌手列表响应（上游 `SingerIndexPageResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerIndexPageResponse {
    /// 歌手列表响应的公共字段。
    #[serde(flatten)]
    pub base: SingerTypeListResponse,
    /// 当前索引筛选值。
    #[serde(default)]
    pub index: i64,
    /// 总数量。
    #[serde(default)]
    pub total: i64,
}

/// 歌手主页基础信息（上游 `HomepageBaseInfo`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct HomepageBaseInfo {
    /// 加密 UIN。
    #[serde(default, alias = "EncryptedUin")]
    pub encrypted_uin: String,
    /// 背景图地址。
    #[serde(default, alias = "BackgroundImage")]
    pub background_image: String,
    /// 头像地址。
    #[serde(default, alias = "Avatar")]
    pub avatar: String,
    /// 展示名称。
    #[serde(default, alias = "Name")]
    pub name: String,
    /// 是否为主页所有者。
    #[serde(default, alias = "IsHost")]
    pub is_host: i64,
    /// 是否为歌手账号。
    #[serde(default, alias = "IsSinger")]
    pub is_singer: i64,
    /// 用户类型标记。
    #[serde(default, alias = "UserType")]
    pub user_type: i64,
}

/// 歌手主页歌手信息（上游 `HomepageSinger`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct HomepageSinger {
    /// 歌手 ID。
    #[serde(default, alias = "SingerID", alias = "singerID")]
    pub id: i64,
    /// 歌手 MID。
    #[serde(default, alias = "SingerMid", alias = "singerMid")]
    pub mid: String,
    /// 歌手名称。
    #[serde(default, alias = "Name", alias = "singerName")]
    pub name: String,
    /// 歌手类型。
    #[serde(default, rename = "type", alias = "SingerType")]
    pub type_: i64,
    /// 歌手图片地址。
    #[serde(default, alias = "SingerPic")]
    pub singer_pic: String,
    /// 歌手图片标识。
    #[serde(default, alias = "SingerPMid")]
    pub singer_pmid: String,
}

/// 主页标签元信息（上游 `TabMeta`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TabMeta {
    /// 标签页 ID。
    #[serde(default, alias = "TabID")]
    pub tab_id: String,
    /// 标签页名称。
    #[serde(default, alias = "TabName")]
    pub tab_name: String,
    /// 标签页标题。
    #[serde(default, alias = "Title")]
    pub title: String,
}

/// 歌手相关专辑条目（上游 `AlbumBrief`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AlbumBrief {
    /// 专辑基础字段。
    #[serde(flatten)]
    pub album: Album,
    /// 曲目数。
    #[serde(default, alias = "totalNum")]
    pub total_num: i64,
    /// 专辑类型文案。
    #[serde(default, alias = "albumType")]
    pub album_type: String,
    /// 歌手名称。
    #[serde(default, alias = "singerName")]
    pub singer_name: String,
    /// 标签列表。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub tags: Vec<String>,
}

/// 歌手视频条目（上游 `VideoBrief`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct VideoBrief {
    /// MV ID。
    #[serde(default, alias = "mvid")]
    pub id: i64,
    /// MV VID。
    #[serde(default)]
    pub vid: String,
    /// MV 类型。
    #[serde(default, rename = "type")]
    pub type_: i64,
    /// 标题。
    #[serde(default)]
    pub title: String,
    /// 封面地址。
    #[serde(default)]
    pub picurl: String,
    /// 封面格式标记。
    #[serde(default)]
    pub picformat: i64,
    /// 时长。
    #[serde(default)]
    pub duration: i64,
    /// 播放量。
    #[serde(default)]
    pub playcnt: i64,
    /// 发布时间戳。
    #[serde(default)]
    pub pubdate: i64,
    /// 图标类型。
    #[serde(default)]
    pub icon_type: i64,
}

/// 歌手主页标签详情响应（上游 `HomepageTabDetailResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct HomepageTabDetailResponse {
    /// 当前标签页 ID。
    #[serde(default, alias = "TabID")]
    pub tab_id: String,
    /// 是否还有更多结果。
    #[serde(default, alias = "HasMore")]
    pub has_more: i64,
    /// 是否需要展示标签。
    #[serde(default, alias = "NeedShowTab")]
    pub need_show_tab: i64,
    /// 排序值。
    #[serde(default, alias = "Order")]
    pub order: i64,
    /// 标签页元信息列表。
    #[serde(
        default,
        alias = "TabList",
        deserialize_with = "crate::models::de_null_as_default"
    )]
    pub tab_list: Vec<TabMeta>,
    /// 简介标签内容（原始列表）。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub introduction_tab: Vec<Value>,
    /// 歌曲标签内容。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub song_tab: Vec<Song>,
    /// 专辑标签内容。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub album_tab: Vec<AlbumBrief>,
    /// 视频标签内容。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub video_tab: Vec<VideoBrief>,
}

/// 歌手主页头部响应（上游 `HomepageHeaderResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct HomepageHeaderResponse {
    /// 状态码。
    #[serde(default, alias = "Status")]
    pub status: i64,
    /// 歌手信息。
    #[serde(default)]
    pub singer: HomepageSinger,
    /// 主页基础信息。
    #[serde(default)]
    pub base_info: HomepageBaseInfo,
    /// 默认标签页详情。
    #[serde(default, alias = "TabDetail")]
    pub tab_detail: HomepageTabDetailResponse,
    /// 附加提示信息。
    #[serde(default, alias = "Prompt")]
    pub prompt: Value,
}

/// 歌手详情基础信息（上游 `SingerBasicInfo`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerBasicInfo {
    /// 歌手 ID。
    #[serde(default, alias = "singer_id")]
    pub id: i64,
    /// 歌手 MID。
    #[serde(default, alias = "singer_mid")]
    pub mid: String,
    /// 歌手名称。
    #[serde(default)]
    pub name: String,
    /// 歌手类型。
    #[serde(default, rename = "type")]
    pub type_: i64,
    /// 图片标识。
    #[serde(default, alias = "singer_pmid")]
    pub pmid: String,
    /// 是否有照片。
    #[serde(default)]
    pub has_photo: i64,
    /// 百科链接。
    #[serde(default)]
    pub wikiurl: String,
}

/// 歌手详情扩展信息（上游 `SingerExtraInfo`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerExtraInfo {
    /// 地区信息。
    #[serde(default, deserialize_with = "crate::models::de_str_or_zero")]
    pub area: String,
    /// 描述文本。
    #[serde(default)]
    pub desc: String,
    /// 标签文本。
    #[serde(default)]
    pub tag: String,
    /// 身份信息。
    #[serde(default, deserialize_with = "crate::models::de_str_or_zero")]
    pub identity: String,
    /// 擅长乐器。
    #[serde(default, deserialize_with = "crate::models::de_str_or_zero")]
    pub instrument: String,
    /// 流派信息。
    #[serde(default, deserialize_with = "crate::models::de_str_or_zero")]
    pub genre: String,
    /// 外文名。
    #[serde(default)]
    pub foreign_name: String,
    /// 生日。
    #[serde(default)]
    pub birthday: String,
    /// 入驻或出道信息。
    #[serde(default, deserialize_with = "crate::models::de_str_or_zero")]
    pub enter: String,
    /// 博客标记。
    #[serde(default, alias = "blogFlag")]
    pub blog_flag: i64,
}

/// 歌手图片地址集合（上游 `SingerPic`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerPic {
    /// 大图（暗色背景）。
    #[serde(default)]
    pub big_black: String,
    /// 大图（亮色背景）。
    #[serde(default)]
    pub big_white: String,
    /// 标准尺寸图片。
    #[serde(default)]
    pub pic: String,
}

/// 歌手相册图片项（上游 `SingerPhotoItem`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerPhotoItem {
    /// 大图地址。
    #[serde(default)]
    pub big: String,
    /// 小图地址。
    #[serde(default)]
    pub small: String,
}

/// 歌手详情条目（上游 `SingerDetail`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerDetail {
    /// 基础信息。
    #[serde(default)]
    pub basic_info: SingerBasicInfo,
    /// 扩展信息。
    #[serde(default)]
    pub ex_info: SingerExtraInfo,
    /// 百科或扩展说明数据。
    #[serde(default)]
    pub wiki: String,
    /// 组合成员列表。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub group_list: Vec<Value>,
    /// 图片地址。
    #[serde(default)]
    pub pic: SingerPic,
    /// 照片列表。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub photos: Vec<SingerPhotoItem>,
    /// 组合附加信息。
    #[serde(default, deserialize_with = "crate::models::de_null_as_default")]
    pub group_info: Vec<Value>,
}

/// 歌手详情响应（上游 `SingerDetailResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerDetailResponse {
    /// 歌手详情列表。
    #[serde(default)]
    pub singer_list: Vec<SingerDetail>,
}

/// 相似歌手条目（上游 `SimilarSinger`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SimilarSinger {
    /// 歌手 ID。
    #[serde(default, alias = "singerId")]
    pub id: i64,
    /// 歌手 MID。
    #[serde(default, alias = "singerMid")]
    pub mid: String,
    /// 歌手名称。
    #[serde(default, alias = "singerName")]
    pub name: String,
    /// 图片标识。
    #[serde(default, alias = "pic_mid")]
    pub pmid: String,
    /// 歌手图片地址。
    #[serde(default, alias = "singerPic")]
    pub singer_pic: String,
    /// 追踪信息。
    #[serde(default)]
    pub trace: String,
    /// 补充文案。
    #[serde(default)]
    pub abt: String,
    /// 附加标记。
    #[serde(default)]
    pub tf: String,
}

/// 相似歌手列表响应（上游 `SimilarSingerResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SimilarSingerResponse {
    /// 相似歌手列表。
    #[serde(default)]
    pub singerlist: Vec<SimilarSinger>,
    /// 返回码。
    #[serde(default)]
    pub code: i64,
    /// 错误消息。
    #[serde(default, alias = "errMsg")]
    pub err_msg: String,
}

/// 歌手歌曲列表响应（上游 `SingerSongListResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerSongListResponse {
    /// 歌手 MID。
    #[serde(default, alias = "singerMid")]
    pub singer_mid: String,
    /// 歌曲总数。
    #[serde(default, alias = "totalNum")]
    pub total_num: i64,
    /// 当前页歌曲列表。
    #[serde(default)]
    pub song_list: Vec<Song>,
}

/// 歌手专辑列表响应（上游 `SingerAlbumListResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerAlbumListResponse {
    /// 歌手 MID。
    #[serde(default, alias = "singerMid")]
    pub singer_mid: String,
    /// 专辑总数。
    #[serde(default)]
    pub total: i64,
    /// 当前页专辑列表。
    #[serde(default, alias = "albumList")]
    pub album_list: Vec<AlbumBrief>,
}

/// 歌手 MV 列表响应（上游 `SingerMvListResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SingerMvListResponse {
    /// MV 总数。
    #[serde(default)]
    pub total: i64,
    /// 当前页 MV 列表。
    #[serde(default, alias = "list")]
    pub mv_list: Vec<VideoBrief>,
}

// ---------------------------------------------------------------------------
// 响应提取辅助（上游 jsonpath 的 Rust 等价物；API 与测试共用）
// ---------------------------------------------------------------------------

/// 提取 `$.Info.Singer` / `$.Info.BaseInfo` 并填充主页头部响应。
pub(crate) fn extract_homepage_info(data: &Value, resp: &mut HomepageHeaderResponse) {
    if let Some(info) = data.get("Info") {
        if let Some(s) = info.get("Singer").cloned() {
            if let Ok(s) = serde_json::from_value::<HomepageSinger>(s) {
                resp.singer = s;
            }
        }
        if let Some(b) = info.get("BaseInfo").cloned() {
            if let Ok(b) = serde_json::from_value::<HomepageBaseInfo>(b) {
                resp.base_info = b;
            }
        }
    }
}

/// 提取主页 Tab 详情各内容列表（`$.IntroductionTab.List` / `$.SongTab.List[*]`
/// / `$.AlbumTab.AlbumList[*]` / `$.VideoTab.VideoList[*]`）。
pub(crate) fn extract_tab_contents(data: &Value, resp: &mut HomepageTabDetailResponse) {
    if let Some(list) = data
        .get("IntroductionTab")
        .and_then(|t| t.get("List"))
        .and_then(|v| v.as_array())
    {
        resp.introduction_tab = list.clone();
    }
    if let Some(list) = data
        .get("SongTab")
        .and_then(|t| t.get("List"))
        .and_then(|v| v.as_array())
    {
        resp.song_tab = list
            .iter()
            .filter_map(|s| serde_json::from_value::<Song>(s.clone()).ok())
            .collect();
    }
    if let Some(list) = data
        .get("AlbumTab")
        .and_then(|t| t.get("AlbumList"))
        .and_then(|v| v.as_array())
    {
        resp.album_tab = list
            .iter()
            .filter_map(|a| serde_json::from_value::<AlbumBrief>(a.clone()).ok())
            .collect();
    }
    if let Some(list) = data
        .get("VideoTab")
        .and_then(|t| t.get("VideoList"))
        .and_then(|v| v.as_array())
    {
        resp.video_tab = list
            .iter()
            .filter_map(|v| serde_json::from_value::<VideoBrief>(v.clone()).ok())
            .collect();
    }
}

/// 提取 `$.songList[*].songInfo` 歌手歌曲列表。
pub(crate) fn extract_singer_songs(data: &Value) -> Vec<Song> {
    data.get("songList")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|item| {
                    item.get("songInfo")
                        .cloned()
                        .and_then(|info| serde_json::from_value::<Song>(info).ok())
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

/// 歌手 API（对应上游 `SingerApi`）。
pub struct SingerApi<'a> {
    client: &'a QqMusicClient,
}

impl<'a> SingerApi<'a> {
    /// 构造歌手 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 获取歌手列表（上游 `get_singer_list`）。
    pub async fn get_singer_list(
        &self,
        area: AreaType,
        sex: SexType,
        genre: GenreType,
    ) -> Result<SingerTypeListResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.musichallSinger.SingerList",
            "GetSingerList",
            json!({
                "hastag": 0,
                "area": area.value(),
                "sex": sex.value(),
                "genre": genre.value(),
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<SingerTypeListResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("singer list 解析失败: {e}")))
    }

    /// 获取按索引分页的歌手列表（上游 `get_singer_list_index`）。
    pub async fn get_singer_list_index(
        &self,
        area: AreaType,
        sex: SexType,
        genre: GenreType,
        index: IndexType,
        page: i64,
        num: i64,
    ) -> Result<SingerIndexPageResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.musichallSinger.SingerList",
            "GetSingerListIndex",
            json!({
                "area": area.value(),
                "sex": sex.value(),
                "genre": genre.value(),
                "index": index.value(),
                "sin": (page - 1) * num,
                "cur_page": page,
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<SingerIndexPageResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("singer list index 解析失败: {e}")))
    }

    /// 获取歌手主页基本信息（上游 `get_info`，固定 Android 平台）。
    pub async fn get_info(&self, mid: &str) -> Result<HomepageHeaderResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.UnifiedHomepage.UnifiedHomepageSrv",
            "GetHomepageHeader",
            json!({"SingerMid": mid}),
        )
        .with_comm(android_comm());
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let mut resp: HomepageHeaderResponse = serde_json::from_value(data.clone())
            .map_err(|e| QqMusicError::InvalidResponse(format!("homepage header 解析失败: {e}")))?;
        extract_homepage_info(&data, &mut resp);
        Ok(resp)
    }

    /// 获取歌手主页特定 Tab 详情（上游 `get_tab_detail`，固定 Android 平台）。
    pub async fn get_tab_detail(
        &self,
        mid: &str,
        tab_type: TabType,
        page: i64,
        num: i64,
    ) -> Result<HomepageTabDetailResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.UnifiedHomepage.UnifiedHomepageSrv",
            "GetHomepageTabDetail",
            json!({
                "SingerMid": mid,
                "IsQueryTabDetail": 1,
                "TabID": tab_type.tab_id(),
                "PageNum": page - 1,
                "PageSize": num,
                "Order": 0,
            }),
        )
        .with_comm(android_comm());
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let mut resp: HomepageTabDetailResponse = serde_json::from_value(data.clone())
            .map_err(|e| QqMusicError::InvalidResponse(format!("homepage tab 解析失败: {e}")))?;
        extract_tab_contents(&data, &mut resp);
        Ok(resp)
    }

    /// 获取歌手描述信息（上游 `get_desc`）。
    ///
    /// 实测该接口的布尔参数必须以 0/1 整数编码，JSON `true` 会返回 10006
    /// （上游直接传 Python bool 属上游缺陷）；故内部统一转为 0/1。
    pub async fn get_desc(
        &self,
        mids: &[String],
        group_singer: bool,
        wiki_singer: bool,
        ex_singer: bool,
        pic: bool,
        photos: bool,
    ) -> Result<SingerDetailResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.musichallSinger.SingerInfoInter",
            "GetSingerDetail",
            json!({
                "singer_mids": mids,
                "group_singer": group_singer as i64,
                "wiki_singer": wiki_singer as i64,
                "ex_singer": ex_singer as i64,
                "pic": pic as i64,
                "photos": photos as i64,
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<SingerDetailResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("singer desc 解析失败: {e}")))
    }

    /// 获取相似歌手列表（上游 `get_similar`）。
    pub async fn get_similar(
        &self,
        mid: &str,
        number: i64,
    ) -> Result<SimilarSingerResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.SimilarSingerSvr",
            "GetSimilarSingerList",
            json!({"singerMid": mid, "number": number}),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<SimilarSingerResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("similar singer 解析失败: {e}")))
    }

    /// 获取歌手的歌曲列表（上游 `get_songs_list`）。
    pub async fn get_songs_list(
        &self,
        mid: &str,
        num: i64,
        page: i64,
    ) -> Result<SingerSongListResponse, QqMusicError> {
        let request = CgiRequest::new(
            "musichall.song_list_server",
            "GetSingerSongList",
            json!({"singerMid": mid, "order": 1, "number": num, "begin": (page - 1) * num}),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let mut resp: SingerSongListResponse = serde_json::from_value(data.clone())
            .map_err(|e| QqMusicError::InvalidResponse(format!("singer songs 解析失败: {e}")))?;
        resp.song_list = extract_singer_songs(&data);
        Ok(resp)
    }

    /// 获取歌手的专辑列表（上游 `get_album_list`）。
    pub async fn get_album_list(
        &self,
        mid: &str,
        num: i64,
        page: i64,
    ) -> Result<SingerAlbumListResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.musichallAlbum.AlbumListServer",
            "GetAlbumList",
            json!({"singerMid": mid, "order": 1, "number": num, "begin": (page - 1) * num}),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<SingerAlbumListResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("singer albums 解析失败: {e}")))
    }

    /// 获取歌手 MV 列表（上游 `get_mv_list`）。
    pub async fn get_mv_list(
        &self,
        mid: &str,
        num: i64,
        page: i64,
    ) -> Result<SingerMvListResponse, QqMusicError> {
        let request = CgiRequest::new(
            "MvService.MvInfoProServer",
            "GetSingerMvList",
            json!({"singermid": mid, "order": 1, "count": num, "start": (page - 1) * num}),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<SingerMvListResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("singer mvs 解析失败: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_values_match_upstream() {
        assert_eq!(AreaType::All.value(), -100);
        assert_eq!(AreaType::China.value(), 200);
        assert_eq!(AreaType::Taiwan.value(), 2);
        assert_eq!(GenreType::Pop.value(), 7);
        assert_eq!(GenreType::RAndB.value(), 11);
        assert_eq!(SexType::Group.value(), 2);
        assert_eq!(IndexType::Letter(b'A').value(), 1);
        assert_eq!(IndexType::Letter(b'Z').value(), 26);
        assert_eq!(IndexType::Hash.value(), 27);
        assert_eq!(TabType::Song.tab_id(), "song_sing");
        assert_eq!(TabType::Album.tab_id(), "album");
    }

    #[test]
    fn parses_real_singer_list_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/singer/list.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: SingerTypeListResponse = serde_json::from_value(data.clone()).unwrap();

        assert_eq!(resp.code, 0);
        assert!(!resp.singerlist.is_empty());
        let s = &resp.singerlist[0];
        assert_eq!(s.name, "周杰伦");
        assert_eq!(s.mid, "0025NhlN2yWrP4");
        assert!(s.id > 0);
        // 服务端 tags 可能为空对象（{}），字段存在即可
        assert!(resp.tags.area.is_empty());
    }

    #[test]
    fn parses_real_singer_index_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/singer/list_index.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: SingerIndexPageResponse = serde_json::from_value(data.clone()).unwrap();

        assert_eq!(resp.base.code, 0);
        assert_eq!(resp.base.singerlist.len(), 80);
        assert!(resp.total > 0);
        assert_eq!(resp.base.singerlist[0].name, "周杰伦");
    }

    #[test]
    fn parses_real_singer_songs_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/singer/songs.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let mut resp: SingerSongListResponse = serde_json::from_value(data.clone()).unwrap();
        resp.song_list = extract_singer_songs(data);

        assert_eq!(resp.singer_mid, "0025NhlN2yWrP4");
        assert!(resp.total_num > 0);
        // 服务端可能忽略 number 返回更多
        assert!(resp.song_list.len() >= 5);
        assert!(!resp.song_list[0].mid.is_empty());
    }

    #[test]
    fn parses_real_singer_albums_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/singer/albums.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: SingerAlbumListResponse = serde_json::from_value(data.clone()).unwrap();

        assert_eq!(resp.singer_mid, "0025NhlN2yWrP4");
        assert!(resp.total > 0);
        // 服务端可能忽略 number 返回更多
        assert!(resp.album_list.len() >= 5);
        assert_eq!(resp.album_list[0].album.name, "周杰伦的床边故事");
    }

    #[test]
    fn parses_real_singer_mvs_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/singer/mvs.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: SingerMvListResponse = serde_json::from_value(data.clone()).unwrap();

        assert!(resp.total > 0);
        assert_eq!(resp.mv_list.len(), 5);
        assert!(!resp.mv_list[0].vid.is_empty());
    }

    #[test]
    fn parses_real_singer_desc_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/singer/desc.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: SingerDetailResponse = serde_json::from_value(data.clone()).unwrap();

        assert_eq!(resp.singer_list.len(), 1);
        let d = &resp.singer_list[0];
        assert_eq!(d.basic_info.name, "周杰伦");
        assert_eq!(d.basic_info.id, 4558);
        assert!(!d.pic.pic.is_empty() || !d.pic.big_black.is_empty());
    }

    #[test]
    fn parses_real_similar_singer_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/singer/similar.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: SimilarSingerResponse = serde_json::from_value(data.clone()).unwrap();

        assert_eq!(resp.code, 0);
        assert_eq!(resp.singerlist.len(), 5);
        assert!(!resp.singerlist[0].mid.is_empty());
    }

    #[test]
    fn parses_real_homepage_header_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/singer/homepage_header.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let mut resp: HomepageHeaderResponse = serde_json::from_value(data.clone()).unwrap();
        extract_homepage_info(data, &mut resp);

        assert_eq!(resp.status, 0);
        assert_eq!(resp.singer.mid, "0025NhlN2yWrP4");
        assert_eq!(resp.singer.name, "周杰伦");
        assert!(!resp.base_info.name.is_empty());
    }

    #[test]
    fn parses_real_tab_detail_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/singer/tab_detail.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let mut resp: HomepageTabDetailResponse = serde_json::from_value(data.clone()).unwrap();
        extract_tab_contents(data, &mut resp);

        assert_eq!(resp.tab_id, "song_sing");
        assert!(resp.has_more >= 0);
        assert_eq!(resp.song_tab.len(), 5);
        assert!(!resp.song_tab[0].mid.is_empty());
    }
}
