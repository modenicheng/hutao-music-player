//! 专辑模块（对应上游 `modules/album.py`）。
//!
//! 详情/歌曲/新碟免登录；收藏/取消收藏需登录态（凭证显式传入）。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::models::{Album, Singer, Song};
use crate::pagination::{Page, Paged, PagedView};
use crate::protocol::cgi::CgiRequest;

/// 专辑详情核心信息（上游 `AlbumDetail`；`GetAlbumDetailResponse.album`，
/// 服务端键 `basicInfo`，基础字段展开自 [`Album`]）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AlbumDetail {
    /// 专辑基础字段（含 subtitle / time_public）。
    #[serde(flatten)]
    pub album: Album,
    /// 专辑简介。
    #[serde(default)]
    pub desc: String,
    /// 专辑语种。
    #[serde(default)]
    pub language: String,
    /// 专辑类型描述。
    #[serde(default, alias = "albumType")]
    pub album_type: String,
    /// 专辑流派文本。
    #[serde(default)]
    pub genre: String,
    /// 百科链接。
    #[serde(default)]
    pub wikiurl: String,
}

/// 发行公司信息（上游 `AlbumCompany`；服务端键 `ID`/`isShow`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AlbumCompany {
    /// 公司 ID。
    #[serde(default, alias = "ID")]
    pub id: i64,
    /// 公司名称。
    #[serde(default)]
    pub name: String,
    /// 是否展示。
    #[serde(default, alias = "isShow")]
    pub is_show: i64,
    /// 公司简介。
    #[serde(default)]
    pub brief: String,
}

/// 专辑详情响应（上游 `GetAlbumDetailResponse`）。
///
/// 实测（2026-09-29）：服务端数据键 `basicInfo`/`company`，署名歌手位于
/// `singer.singerList`（由 [`extract_singers`] 填充）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GetAlbumDetailResponse {
    /// 专辑基础信息与补充描述。
    #[serde(default, alias = "basicInfo")]
    pub album: AlbumDetail,
    /// 发行公司信息。
    #[serde(default)]
    pub company: AlbumCompany,
    /// 专辑署名歌手列表。
    #[serde(default)]
    pub singers: Vec<Singer>,
}

/// 专辑歌曲列表响应（上游 `GetAlbumSongResponse`）。
///
/// 实测（2026-09-29）：服务端数据键 `albumMid`/`totalNum`，歌曲位于
/// `songList[*].songInfo`（由 [`extract_song_list`] 填充）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GetAlbumSongResponse {
    /// 专辑 MID。
    #[serde(default, alias = "albumMid")]
    pub album_mid: String,
    /// 歌曲总数。
    #[serde(default, alias = "totalNum")]
    pub total_num: i64,
    /// 当前页歌曲列表。
    #[serde(default)]
    pub song_list: Vec<Song>,
}

/// 新碟上架条目（上游 `NewAlbumItem`；基础字段展开自 [`Album`]）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct NewAlbumItem {
    /// 专辑基础字段。
    #[serde(flatten)]
    pub album: Album,
    /// 署名歌手列表。
    #[serde(default)]
    pub singers: Vec<Singer>,
    /// 发行日期（YYYY-MM-DD）。
    #[serde(default)]
    pub release_time: String,
    /// 专辑类型。
    #[serde(default, rename = "type")]
    pub type_: i64,
    /// 地区标识。
    #[serde(default)]
    pub area: i64,
    /// 流派标识。
    #[serde(default)]
    pub genre: i64,
    /// 语种标识。
    #[serde(default)]
    pub language: i64,
}

/// 新碟上架响应（上游 `GetNewAlbumResponse`）。
///
/// 实测（2026-09-29）：服务端数据键 `total`/`albums`。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GetNewAlbumResponse {
    /// 该地区新碟总数。
    #[serde(default)]
    pub total: i64,
    /// 当前页新碟列表。
    #[serde(default)]
    pub albums: Vec<NewAlbumItem>,
}

/// 收藏/取消收藏专辑响应（上游 `AlbumFavWriteResponse`）。
///
/// 实测（2026-09-29）：判定字段 `result`/`v_failedAlbumId` 位于**内层
/// data**（另含 `reason`/`v_failedAlbumMid`/`imported_album_cnt`/
/// `order_album_cnt`），键形与上游一致。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AlbumFavWriteResponse {
    /// 操作结果码（0=成功）。
    #[serde(default)]
    pub result: i64,
    /// 失败专辑 ID 列表。
    #[serde(default, alias = "v_failedAlbumId")]
    pub failed_album_id: Vec<i64>,
}

impl AlbumFavWriteResponse {
    /// 是否操作成功（result=0 且无失败项）。
    pub fn success(&self) -> bool {
        self.result == 0 && self.failed_album_id.is_empty()
    }
}

// ---------------------------------------------------------------------------
// 统一分页视图（has_more 归一规则见 crate::pagination 模块文档）
// ---------------------------------------------------------------------------

/// 无显式 hasmore 字段，按总数推算（规则 2）：`total_num > offset + len`。
impl Paged for GetAlbumSongResponse {
    type Item = Song;

    fn paged(&self, page: Page) -> PagedView<'_, Song> {
        PagedView {
            items: &self.song_list,
            total: self.total_num,
            has_more: self.total_num > page.offset() as i64 + self.song_list.len() as i64,
            page,
        }
    }
}

/// 无显式 hasmore 字段，按总数推算（规则 2）：`total > offset + len`。
impl Paged for GetNewAlbumResponse {
    type Item = NewAlbumItem;

    fn paged(&self, page: Page) -> PagedView<'_, NewAlbumItem> {
        PagedView {
            items: &self.albums,
            total: self.total,
            has_more: self.total > page.offset() as i64 + self.albums.len() as i64,
            page,
        }
    }
}

/// 提取 `$.singer.singerList` 歌手列表（上游 jsonpath，API 与测试共用）。
pub(crate) fn extract_singers(data: &Value) -> Vec<Singer> {
    data.get("singer")
        .and_then(|s| s.get("singerList"))
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|s| serde_json::from_value::<Singer>(s.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// 提取 `$.songList[*].songInfo` 歌曲列表（上游 jsonpath，API 与测试共用）。
pub(crate) fn extract_song_list(data: &Value) -> Vec<Song> {
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

/// 专辑 API（对应上游 `AlbumApi`）。
pub struct AlbumApi<'a> {
    client: &'a QqMusicClient,
}

impl<'a> AlbumApi<'a> {
    /// 构造专辑 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 获取专辑详细信息（上游 `get_detail`；登录：免登录）。
    ///
    /// `value` 为专辑数字 ID（全数字，服务端键 `albumId`）或专辑 MID
    /// （服务端键 `albumMId`，注意大小写）。
    ///
    /// 实测（2026-09-29）：可用（免登录），MID 与数字 ID 两种入参均验证通过。
    pub async fn get_detail(&self, value: &str) -> Result<GetAlbumDetailResponse, QqMusicError> {
        let param = if value.chars().all(|c| c.is_ascii_digit()) {
            json!({"albumId": value.parse::<i64>().unwrap_or(0)})
        } else {
            json!({"albumMId": value})
        };
        let request = CgiRequest::new(
            "music.musichallAlbum.AlbumInfoServer",
            "GetAlbumDetail",
            param,
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let mut resp: GetAlbumDetailResponse =
            serde_json::from_value(data.clone()).map_err(|e| {
                QqMusicError::InvalidResponse(format!("failed to parse album detail: {e}"))
            })?;
        resp.singers = extract_singers(&data);
        Ok(resp)
    }

    /// 获取专辑歌曲列表（上游 `get_song`；登录：免登录）。
    ///
    /// `value` 为专辑数字 ID 或 MID（同 [`Self::get_detail`]）；`page` 为
    /// 分页窗口（服务端 `begin = page.offset()`、`num = page.num`）。
    ///
    /// 实测（2026-09-29）：可用（免登录），返回 `totalNum=10`（实际曲数）
    /// 与请求页歌曲。
    pub async fn get_song(
        &self,
        value: &str,
        page: Page,
    ) -> Result<GetAlbumSongResponse, QqMusicError> {
        let mut param = json!({"begin": page.offset(), "num": page.num});
        if value.chars().all(|c| c.is_ascii_digit()) {
            param["albumId"] = json!(value.parse::<i64>().unwrap_or(0));
        } else {
            param["albumMid"] = json!(value);
        }
        let request = CgiRequest::new(
            "music.musichallAlbum.AlbumSongList",
            "GetAlbumSongList",
            param,
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let mut resp: GetAlbumSongResponse = serde_json::from_value(data.clone()).map_err(|e| {
            QqMusicError::InvalidResponse(format!("failed to parse album songs: {e}"))
        })?;
        resp.song_list = extract_song_list(&data);
        Ok(resp)
    }

    /// 获取新碟上架列表（上游 `get_new_album`；登录：免登录）。
    ///
    /// `area`: 1=内地, 2=港台, 3=欧美, 4=韩国, 5=日本, 6=其他；`page` 为
    /// 分页窗口（服务端 `num = page.num`、`start = page.offset()`）。
    ///
    /// 实测（2026-09-29）：可用（免登录）。
    pub async fn get_new_album(
        &self,
        area: i64,
        page: Page,
    ) -> Result<GetNewAlbumResponse, QqMusicError> {
        let request = CgiRequest::new(
            "newalbum.NewAlbumServer",
            "get_new_album_info",
            json!({"area": area, "num": page.num, "start": page.offset()}),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<GetNewAlbumResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("failed to parse new album: {e}")))
    }

    /// 收藏专辑（上游 `fav_album`；登录：需登录）。
    ///
    /// `album_id` 为专辑**数字 ID** 数组（非 MID；服务端键 `v_albumId`）。
    /// 成功判定见 [`AlbumFavWriteResponse::success`]。
    ///
    /// 实测（2026-09-29）：可用（净零闭环验证：fav 新碟 → del 恢复）。
    pub async fn fav_album(
        &self,
        album_id: &[i64],
        credential: &Credential,
    ) -> Result<AlbumFavWriteResponse, QqMusicError> {
        self.fav_write("FavAlbum", album_id, credential).await
    }

    /// 取消收藏专辑（上游 `del_fav_album`；登录：需登录）。
    ///
    /// `album_id` 为专辑**数字 ID** 数组（非 MID；服务端键 `v_albumId`）。
    ///
    /// 实测（2026-09-29）：可用（净零闭环验证；未收藏/已取消均 `result=0`）。
    pub async fn del_fav_album(
        &self,
        album_id: &[i64],
        credential: &Credential,
    ) -> Result<AlbumFavWriteResponse, QqMusicError> {
        self.fav_write("CancelFavAlbum", album_id, credential).await
    }

    async fn fav_write(
        &self,
        method: &str,
        album_id: &[i64],
        credential: &Credential,
    ) -> Result<AlbumFavWriteResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.musicasset.AlbumFavWrite",
            method,
            json!({"v_albumId": album_id}),
        )
        .with_require_login(true);
        let data = self
            .client
            .musicu_request(&request, Some(credential))
            .await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<AlbumFavWriteResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("failed to parse album fav: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_album_detail_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/album/detail.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let mut resp: GetAlbumDetailResponse = serde_json::from_value(data.clone()).unwrap();
        resp.singers = extract_singers(data);

        assert_eq!(resp.album.album.id, 1458791);
        assert_eq!(resp.album.album.name, "周杰伦的床边故事");
        assert!(!resp.album.album.mid.is_empty());
        assert!(!resp.company.name.is_empty(), "company name");
        assert!(
            !resp.singers.is_empty(),
            "singers extracted from $.singer.singerList"
        );
        assert_eq!(resp.singers[0].name, "周杰伦");
    }

    #[test]
    fn parses_real_album_songs_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/album/songs.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let mut resp: GetAlbumSongResponse = serde_json::from_value(data.clone()).unwrap();
        resp.song_list = extract_song_list(data);

        assert_eq!(resp.total_num, 10);
        assert_eq!(resp.song_list.len(), 5);
        assert!(!resp.song_list[0].mid.is_empty());
        assert!(!resp.song_list[0].name.is_empty());
    }

    #[test]
    fn parses_real_new_album_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/album/new.json");
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: GetNewAlbumResponse = serde_json::from_value(data.clone()).unwrap();

        assert!(resp.total > 0);
        assert_eq!(resp.albums.len(), 5);
        let a = &resp.albums[0];
        assert!(!a.album.mid.is_empty());
        assert!(!a.album.name.is_empty());
    }

    #[test]
    fn fav_write_success_helper() {
        let resp = AlbumFavWriteResponse {
            result: 0,
            failed_album_id: vec![],
        };
        assert!(resp.success());
        let fail = AlbumFavWriteResponse {
            result: 0,
            failed_album_id: vec![123],
        };
        assert!(!fail.success());
    }
}
