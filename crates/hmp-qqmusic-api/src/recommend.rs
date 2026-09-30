//! 推荐模块（对应上游 `modules/recommend.py`）。
//!
//! 首页 Feed / 雷达推荐 / 推荐歌单 / 推荐新歌免登录；
//! 「猜你喜欢」在非 Android 平台需要登录态（凭证显式传入）。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::models::Song;
use crate::pagination::{Page, Paged, PagedView, UNKNOWN_TOTAL};
use crate::protocol::cgi::CgiRequest;

/// 首页推荐细分卡片分组（上游 `RecommendNiche`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RecommendNiche {
    /// 细分分组 ID。
    #[serde(default)]
    pub id: i64,
    /// 标题模板。
    #[serde(default)]
    pub title_template: String,
    /// 标题实际展示内容。
    #[serde(default)]
    pub title_content: String,
    /// 原始卡片列表（服务端键 `v_card`）。
    #[serde(default, alias = "v_card")]
    pub cards: Vec<Value>,
}

/// 首页推荐楼层（上游 `RecommendShelf`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RecommendShelf {
    /// 楼层 ID。
    #[serde(default)]
    pub id: i64,
    /// 楼层标题模板。
    #[serde(default)]
    pub title_template: String,
    /// 楼层标题实际展示内容。
    #[serde(default)]
    pub title_content: String,
    /// 更多入口信息。
    #[serde(default)]
    pub more: Value,
    /// 楼层下属细分分组列表（服务端键 `v_niche`）。
    #[serde(default, alias = "v_niche")]
    pub niches: Vec<RecommendNiche>,
}

/// 首页推荐首屏响应（上游 `RecommendFeedCardResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RecommendFeedCardResponse {
    /// 接口返回码。
    #[serde(default)]
    pub retcode: i64,
    /// 附加消息。
    #[serde(default)]
    pub msg: String,
    /// 提示信息。
    #[serde(default)]
    pub prompt: String,
    /// 分页或批次计数。
    #[serde(default)]
    pub d_num: i64,
    /// 继续加载标记。
    #[serde(default)]
    pub load_mark: i64,
    /// 推荐楼层列表（服务端键 `v_shelf`）。
    #[serde(default, alias = "v_shelf")]
    pub shelves: Vec<RecommendShelf>,
}

/// 「猜你喜欢」响应（上游 `GuessRecommendResponse`）。
///
/// 实测（2026-09-29）：服务端数据键为 `data.tracks`（与上游别名一致），
/// 每项为完整 Song 形态。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GuessRecommendResponse {
    /// 推荐歌曲列表。
    #[serde(default, alias = "tracks")]
    pub songs: Vec<Song>,
}

/// 雷达推荐响应（上游 `RadarRecommendResponse`）。
///
/// 实测（2026-09-29）：服务端数据键 `VecSongs[*].Track`（歌曲）、
/// `RecommendSongIds`/`BaseSongIds`/`HasMore`/`Toast`/`TimeStamp` 均按上游别名解析。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RadarRecommendResponse {
    /// 推荐歌曲列表。
    #[serde(default)]
    pub songs: Vec<Song>,
    /// 推荐歌曲 ID 列表。
    #[serde(default, alias = "RecommendSongIds")]
    pub recommend_song_ids: Vec<i64>,
    /// 作为推荐依据的基础歌曲 ID 列表。
    #[serde(default, alias = "BaseSongIds")]
    pub base_song_ids: Vec<i64>,
    /// 是否还能继续获取更多推荐。
    #[serde(default, alias = "HasMore")]
    pub has_more: bool,
    /// 提示信息。
    #[serde(default, alias = "Toast")]
    pub toast: String,
    /// 服务端时间戳。
    #[serde(default, alias = "TimeStamp")]
    pub timestamp: i64,
    /// 关联视频卡片数据。
    #[serde(
        default,
        alias = "VideoCards",
        deserialize_with = "crate::models::de_null_as_default"
    )]
    pub video_cards: Value,
}

/// 推荐歌单条目（上游 `RecommendSonglistItem`）。
///
/// 由 [`extract_songlists`] 从 `$.List[*].Playlist.basic` 解析；`cover`/
/// `creator` 也位于 `basic` 下（2026-09-29 实测）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RecommendSonglistItem {
    /// 歌单数字 ID。
    #[serde(default, alias = "tid", alias = "dissid")]
    pub id: i64,
    /// 目录 ID。
    #[serde(default, alias = "dirId")]
    pub dirid: i64,
    /// 歌单标题。
    #[serde(default, alias = "dissname", alias = "dirName")]
    pub title: String,
    /// 歌单封面地址（服务端键 `basic.cover.default_url`，由提取函数填充）。
    #[serde(default)]
    pub picurl: String,
    /// 歌单简介。
    #[serde(default)]
    pub desc: String,
    /// 歌曲数量。
    #[serde(default, alias = "songNum", alias = "song_cnt")]
    pub songnum: i64,
    /// 播放量。
    #[serde(default, alias = "playCnt", alias = "play_cnt")]
    pub listennum: i64,
    /// 创建者昵称（服务端键 `basic.creator.nick`，由提取函数填充）。
    #[serde(default)]
    pub creator_nick: String,
}

/// 推荐歌单分页响应（上游 `RecommendSonglistResponse`）。
///
/// 实测（2026-09-29）：服务端数据键 `List`/`HasMore`/`FromLimit`/`Msg`。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RecommendSonglistResponse {
    /// 当前批次推荐歌单列表。
    #[serde(default)]
    pub songlists: Vec<RecommendSonglistItem>,
    /// 是否还能继续拉取。
    #[serde(default, alias = "HasMore")]
    pub has_more: bool,
    /// 当前批次偏移。
    #[serde(default, alias = "FromLimit")]
    pub from_limit: i64,
    /// 附加消息。
    #[serde(default, alias = "Msg")]
    pub msg: String,
}

/// 推荐新歌页标签项（上游 `RecommendNewSongTag`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RecommendNewSongTag {
    /// 标签记录 ID。
    #[serde(default)]
    pub id: i64,
    /// 标签 ID。
    #[serde(default)]
    pub tagid: i64,
    /// 标签名称。
    #[serde(default)]
    pub tag: String,
    /// 标签跳转链接。
    #[serde(default)]
    pub link: String,
    /// 标签来源类型。
    #[serde(default)]
    pub from_type: i64,
}

/// 推荐新歌响应（上游 `RecommendNewSongResponse`）。
///
/// 实测（2026-09-29）：服务端数据键 `lanlist`/`lan`/`songlist`/`ret_msg`/
/// `type`/`songTagInfoList`；`songlist` 为完整 Song 形态。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RecommendNewSongResponse {
    /// 可选语言或频道列表。
    #[serde(default)]
    pub lanlist: Vec<Value>,
    /// 当前语言或频道标识。
    #[serde(default)]
    pub lan: String,
    /// 当前新歌列表。
    #[serde(default, alias = "songlist")]
    pub songs: Vec<Song>,
    /// 附加返回消息。
    #[serde(default)]
    pub ret_msg: String,
    /// 当前推荐类型标记。
    #[serde(default, rename = "type")]
    pub type_: i64,
    /// 新歌标签列表。
    #[serde(default, alias = "songTagInfoList")]
    pub song_tags: Vec<RecommendNewSongTag>,
}

/// 提取 `$.List[*].Playlist.basic` 推荐歌单列表，并填充
/// `basic.cover.default_url` / `basic.creator.nick`（上游 jsonpath 以提取项
/// `basic` 为根求值；API 与测试共用）。
///
/// 实测（2026-09-29）：`cover`/`creator` 位于 `Playlist.basic` 下
/// （`Playlist` 直下无这两个键）；此前误读 `Playlist.cover` 层级导致
/// `picurl`/`creator_nick` 恒为空（daemon 发现页无封面），已修复。
pub(crate) fn extract_songlists(data: &Value) -> Vec<RecommendSonglistItem> {
    data.get("List")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|item| {
                    let playlist = item.get("Playlist")?;
                    let basic = playlist.get("basic")?;
                    let mut sl: RecommendSonglistItem =
                        serde_json::from_value(basic.clone()).ok()?;
                    // 上游 jsonpath `$.cover.default_url` / `$.creator.nick`
                    // 相对 basic 求值；Playlist 直下留作兼容回退。
                    if let Some(cover) = basic
                        .get("cover")
                        .or_else(|| playlist.get("cover"))
                        .and_then(|c| c.get("default_url"))
                        .and_then(|v| v.as_str())
                    {
                        sl.picurl = cover.to_owned();
                    }
                    if let Some(nick) = basic
                        .get("creator")
                        .or_else(|| playlist.get("creator"))
                        .and_then(|c| c.get("nick"))
                        .and_then(|v| v.as_str())
                    {
                        sl.creator_nick = nick.to_owned();
                    }
                    Some(sl)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 提取 `$.VecSongs[*].Track` 雷达推荐歌曲列表（上游 jsonpath）。
pub(crate) fn extract_radar_songs(data: &Value) -> Vec<Song> {
    data.get("VecSongs")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|item| {
                    item.get("Track")
                        .cloned()
                        .and_then(|t| serde_json::from_value::<Song>(t).ok())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 推荐 API（对应上游 `RecommendApi`）。
pub struct RecommendApi<'a> {
    client: &'a QqMusicClient,
}

// ---------------------------------------------------------------------------
// 统一分页视图（has_more 归一规则见 crate::pagination 模块文档）
// ---------------------------------------------------------------------------

/// 显式 hasmore 字段（`HasMore` 布尔），以服务端为准（规则 1）。
///
/// 服务端不返回总数（`total` 为 [`UNKNOWN_TOTAL`]）；上游翻页以
/// `from_limit` 为游标（`CursorStrategy`）。
impl Paged for RecommendSonglistResponse {
    type Item = RecommendSonglistItem;

    fn paged(&self, page: Page) -> PagedView<'_, RecommendSonglistItem> {
        PagedView {
            items: &self.songlists,
            total: UNKNOWN_TOTAL,
            has_more: self.has_more,
            page,
        }
    }
}

impl<'a> RecommendApi<'a> {
    /// 构造推荐 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 获取首页推荐 Feed 首屏（上游 `get_home_feed`；登录：免登录）。
    ///
    /// `page` 页码（从 1 开始）；`direction` 刷新方向（0=刷新，1=加载更多）；
    /// `s_num` 已加载楼层数；`v_cache` 已曝光楼层 ID 缓存（防重复推荐）。
    /// 翻页时调用方需回传上一响应的楼层 ID 集合并递增 `s_num`（上游
    /// `MultiFieldContinuationStrategy` 语义）。
    ///
    /// 实测（2026-09-29）：可用（免登录），首屏 8 个楼层，`v_shelf`/`v_niche`
    /// 键形与上游一致。
    pub async fn get_home_feed(
        &self,
        page: i64,
        direction: i64,
        s_num: i64,
        v_cache: &[i64],
    ) -> Result<RecommendFeedCardResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.recommend.RecommendFeed",
            "get_recommend_feed",
            json!({
                "direction": direction,
                "page": page,
                "s_num": s_num,
                "v_cache": v_cache,
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<RecommendFeedCardResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("failed to parse home feed: {e}")))
    }

    /// 获取「猜你喜欢」私人电台推荐（上游 `get_guess_recommend`；登录：需登录）。
    ///
    /// 参数固定（电台 id=99、5 首），本方法不接受自定义参数。
    ///
    /// 实测（2026-09-29）：需登录态可用（返回 5 首，数据键 `data.tracks`）；
    /// 非 Android 平台免登录实测返回 CGI 错误 1000（上游提示一致）。
    pub async fn get_guess_recommend(
        &self,
        credential: &Credential,
    ) -> Result<GuessRecommendResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.radioProxy.MbTrackRadioSvr",
            "get_radio_track",
            json!({"id": 99, "num": 5, "from": 0, "scene": 0, "song_ids": []}),
        )
        .with_require_login(true);
        let data = self
            .client
            .musicu_request(&request, Some(credential))
            .await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<GuessRecommendResponse>(data).map_err(|e| {
            QqMusicError::InvalidResponse(format!("failed to parse guess recommend: {e}"))
        })
    }

    /// 获取雷达推荐歌曲（上游 `get_radar_recommend`；登录：登录增强）。
    ///
    /// `page` 页码（从 1 开始）；`HasMore` 为真时可继续翻页。
    ///
    /// 实测（2026-09-29）：免登录可用（未登录时基于服务端默认推荐，
    /// toast 提示「根据您的收藏发现了10首歌曲」），`VecSongs[*].Track` 解析正常。
    pub async fn get_radar_recommend(
        &self,
        page: i64,
    ) -> Result<RadarRecommendResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.recommend.TrackRelationServer",
            "GetRadarSong",
            json!({"Page": page, "ReqType": 0, "FavSongs": [], "EntranceSongs": []}),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let mut resp: RadarRecommendResponse = serde_json::from_value(data.clone())
            .map_err(|e| QqMusicError::InvalidResponse(format!("failed to parse radar: {e}")))?;
        resp.songs = extract_radar_songs(&data);
        Ok(resp)
    }

    /// 获取推荐歌单广场（上游 `get_recommend_songlist`；登录：免登录）。
    ///
    /// `page` 为分页窗口（服务端 `From = page.offset()`、`Size = page.num`；
    /// 上游默认每页 25）。`HasMore` 为真时可继续翻页（响应的
    /// `from_limit` 为上游 `CursorStrategy` 的续页游标，页号窗口亦可复用）。
    ///
    /// 实测（2026-09-29）：可用（免登录）；歌单封面/创建者位于
    /// `List[*].Playlist.basic` 下（此前提取层级错误致 `picurl`/`creator_nick`
    /// 恒为空，已修复）。
    pub async fn get_recommend_songlist(
        &self,
        page: Page,
    ) -> Result<RecommendSonglistResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.playlist.PlaylistSquare",
            "GetRecommendFeed",
            json!({"From": page.offset(), "Size": page.num}),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let mut resp: RecommendSonglistResponse =
            serde_json::from_value(data.clone()).map_err(|e| {
                QqMusicError::InvalidResponse(format!("failed to parse recommend songlist: {e}"))
            })?;
        resp.songlists = extract_songlists(&data);
        Ok(resp)
    }

    /// 获取推荐新歌（上游 `get_recommend_newsong`；登录：免登录）。
    ///
    /// `new_song_type` 地区/语种：1=内地, 2=欧美, 3=日本, 4=韩国, 5=最新, 6=港台。
    ///
    /// 实测（2026-09-29）：可用（免登录，type=5 返回 58 首），数据键
    /// `lanlist`/`lan`/`songlist`/`ret_msg`/`type`/`songTagInfoList`。
    pub async fn get_recommend_newsong(
        &self,
        new_song_type: i64,
    ) -> Result<RecommendNewSongResponse, QqMusicError> {
        let request = CgiRequest::new(
            "newsong.NewSongServer",
            "get_new_song_info",
            json!({"type": new_song_type}),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<RecommendNewSongResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("failed to parse new song: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_recommend_songlist_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/recommend/songlist.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let mut resp: RecommendSonglistResponse = serde_json::from_value(data.clone()).unwrap();
        resp.songlists = extract_songlists(data);

        assert!(resp.has_more);
        assert!(!resp.songlists.is_empty());
        let sl = &resp.songlists[0];
        assert!(sl.id > 0, "playlist tid as id");
        assert!(!sl.title.is_empty());
        // 回归锚点（2026-09-29 修复）：cover/creator 位于 Playlist.basic 下，
        // 此前提取 Playlist.cover 层级导致 picurl/creator_nick 恒为空。
        assert!(!sl.picurl.is_empty(), "basic.cover.default_url 应解析出封面");
        assert!(!sl.creator_nick.is_empty(), "basic.creator.nick 应解析出昵称");
    }

    #[test]
    fn extract_songlists_merges_cover_and_creator() {
        // 合成数据：覆盖 cover.default_url / creator.nick 提取路径
        let raw = json!({
            "List": [
                {
                    "Playlist": {
                        "basic": {
                            "tid": 9282300617i64,
                            "dirid": 0,
                            "title": "测试歌单",
                            "desc": "",
                            "song_cnt": 12,
                            "play_cnt": 3456
                        },
                        "cover": {"default_url": "https://y.gtimg.cn/music/photo_new/T001R500x500M000abc.jpg"},
                        "creator": {"nick": "测试用户"}
                    }
                },
                {
                    "Playlist": {"basic": {"tid": 2, "title": "无封面歌单"}}
                }
            ]
        });
        let list = extract_songlists(&raw);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, 9282300617i64);
        assert_eq!(list[0].title, "测试歌单");
        assert_eq!(list[0].songnum, 12);
        assert_eq!(list[0].listennum, 3456);
        assert_eq!(
            list[0].picurl,
            "https://y.gtimg.cn/music/photo_new/T001R500x500M000abc.jpg"
        );
        assert_eq!(list[0].creator_nick, "测试用户");
        // 无 cover/creator 的歌单保持默认
        assert_eq!(list[1].picurl, "");
        assert_eq!(list[1].creator_nick, "");
    }

    #[test]
    fn parses_real_home_feed_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/recommend/feed.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: RecommendFeedCardResponse = serde_json::from_value(data.clone()).unwrap();

        assert_eq!(resp.retcode, 0);
        assert!(!resp.shelves.is_empty());
        assert!(resp.shelves[0].id > 0);
        assert!(!resp.shelves[0].niches.is_empty(), "v_niche present");
    }

    #[test]
    fn parses_real_radar_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/recommend/radar.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let mut resp: RadarRecommendResponse = serde_json::from_value(data.clone()).unwrap();
        resp.songs = extract_radar_songs(data);

        assert_eq!(resp.songs.len(), 10);
        assert!(resp.has_more);
        assert!(!resp.songs[0].mid.is_empty());
    }

    #[test]
    fn parses_real_newsong_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/recommend/newsong.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: RecommendNewSongResponse = serde_json::from_value(data.clone()).unwrap();

        assert_eq!(resp.type_, 5);
        assert_eq!(resp.songs.len(), 60);
        assert!(!resp.songs[0].mid.is_empty());
        assert!(!resp.song_tags.is_empty());
    }
}
