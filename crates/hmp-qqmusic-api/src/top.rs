//! 排行榜模块（对应上游 `modules/top.py`）。
//!
//! 分类与详情均免登录。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::error::QqMusicError;
use crate::models::Song;
use crate::protocol::cgi::CgiRequest;

/// 排行榜预览歌曲条目（上游 `TopPreviewSong`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TopPreviewSong {
    /// 排名位置。
    #[serde(default)]
    pub rank: i64,
    /// 排名变化类型。
    #[serde(default, alias = "rankType")]
    pub rank_type: i64,
    /// 排名变化值文本。
    #[serde(default, alias = "rankValue")]
    pub rank_value: String,
    /// 歌曲数字 ID。
    #[serde(default, alias = "songId")]
    pub id: i64,
    /// 歌曲标题。
    #[serde(default, alias = "title")]
    pub name: String,
    /// 歌手名称文本。
    #[serde(default, alias = "singerName")]
    pub singer_name: String,
    /// 主歌手 MID。
    #[serde(default, alias = "singerMid")]
    pub singer_mid: String,
    /// 专辑 MID。
    #[serde(default, alias = "albumMid")]
    pub album_mid: String,
    /// 封面地址。
    #[serde(default)]
    pub cover: String,
    /// MV 数字 ID。
    #[serde(default, alias = "mvid")]
    pub mv_id: i64,
}

/// 排行榜摘要信息（上游 `TopSummary`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TopSummary {
    /// 排行榜 ID。
    #[serde(default, alias = "topId")]
    pub id: i64,
    /// 榜单标题。
    #[serde(default, alias = "title")]
    pub name: String,
    /// 榜单完整标题。
    #[serde(default, alias = "titleDetail")]
    pub title_detail: String,
    /// 榜单副标题。
    #[serde(default, alias = "titleSub")]
    pub title_sub: String,
    /// 榜单简介。
    #[serde(default)]
    pub intro: String,
    /// 榜单期数。
    #[serde(default)]
    pub period: String,
    /// 更新时间。
    #[serde(default, alias = "updateTime")]
    pub update_time: String,
    /// 播放量。
    #[serde(default, alias = "listenNum")]
    pub listen_num: i64,
    /// 榜单总曲数。
    #[serde(default, alias = "totalNum")]
    pub total_num: i64,
    /// 榜单预览歌曲。
    #[serde(default, alias = "song")]
    pub songs: Vec<TopPreviewSong>,
    /// 榜单封面。
    #[serde(default, alias = "frontPicUrl")]
    pub front_pic_url: String,
    /// 榜单头图。
    #[serde(default, alias = "headPicUrl")]
    pub head_pic_url: String,
    /// H5 跳转地址。
    #[serde(default, alias = "h5JumpUrl")]
    pub h5_jump_url: String,
    /// 客户端跳转 Scheme。
    #[serde(default, alias = "specialScheme")]
    pub special_scheme: String,
}

/// 排行榜分类（上游 `TopCategory`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TopCategory {
    /// 分类 ID。
    #[serde(default, alias = "groupId")]
    pub id: i64,
    /// 分类名称。
    #[serde(default, alias = "groupName")]
    pub name: String,
    /// 分类下的排行榜摘要列表。
    #[serde(default)]
    pub toplist: Vec<TopSummary>,
}

/// 排行榜分类响应（上游 `TopCategoryResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TopCategoryResponse {
    /// 排行榜分类列表。
    #[serde(default)]
    pub group: Vec<TopCategory>,
}

/// 排行榜详情响应（上游 `TopDetailResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TopDetailResponse {
    /// 排行榜基础信息。
    #[serde(default, alias = "data")]
    pub info: TopSummary,
    /// 排行榜歌曲列表。
    #[serde(default, alias = "songInfoList")]
    pub songs: Vec<Song>,
    /// 歌曲标签列表。
    #[serde(
        default,
        alias = "songTagInfoList",
        deserialize_with = "crate::models::de_null_as_default"
    )]
    pub song_tags: Vec<Value>,
    /// 附加信息列表。
    #[serde(
        default,
        alias = "extInfoList",
        deserialize_with = "crate::models::de_null_as_default"
    )]
    pub ext_info_list: Vec<Value>,
    /// 榜单索引信息列表。
    #[serde(
        default,
        alias = "indexInfoList",
        deserialize_with = "crate::models::de_null_as_default"
    )]
    pub index_info_list: Vec<Value>,
}

/// 排行榜 API（对应上游 `TopApi`）。
pub struct TopApi<'a> {
    client: &'a QqMusicClient,
}

impl<'a> TopApi<'a> {
    /// 构造排行榜 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 获取所有排行榜分类（上游 `get_category`）。
    pub async fn get_category(&self) -> Result<TopCategoryResponse, QqMusicError> {
        let request = CgiRequest::new("music.musicToplist.Toplist", "GetAll", json!({}));
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<TopCategoryResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("top category 解析失败: {e}")))
    }

    /// 获取排行榜详情及其歌曲列表（上游 `get_detail`）。
    pub async fn get_detail(
        &self,
        top_id: i64,
        num: i64,
        page: i64,
        tag: bool,
    ) -> Result<TopDetailResponse, QqMusicError> {
        let mut param = json!({"topId": top_id, "offset": num * (page - 1), "num": num});
        if tag {
            param["withTags"] = json!(true);
        }
        let request = CgiRequest::new("music.musicToplist.Toplist", "GetDetail", param);
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<TopDetailResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("top detail 解析失败: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_top_category_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/top/category.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: TopCategoryResponse = serde_json::from_value(data.clone()).unwrap();

        assert!(!resp.group.is_empty());
        let g = &resp.group[0];
        assert!(!g.name.is_empty());
        assert!(!g.toplist.is_empty());
        let t = &g.toplist[0];
        assert!(t.id > 0);
        assert!(!t.name.is_empty());
    }

    #[test]
    fn parses_real_top_detail_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/top/detail.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: TopDetailResponse = serde_json::from_value(data.clone()).unwrap();

        assert_eq!(resp.info.id, 62);
        assert_eq!(resp.info.name, "飙升榜");
        assert!(resp.info.total_num > 0);
        assert_eq!(resp.songs.len(), 5);
        assert!(!resp.songs[0].mid.is_empty());
        assert!(!resp.songs[0].name.is_empty());
    }
}
