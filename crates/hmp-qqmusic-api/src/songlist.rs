//! 歌单模块（对应上游 `modules/songlist.py`）。
//!
//! `get_detail` 免登录；创建/删除/加歌/删歌/收藏均需登录态，
//! 凭证由调用方显式传入 `&Credential`（§6.4 凭证解耦）。
//!
//! 写操作 ID 速查：`create`/`delete`/`add_songs`/`del_songs` 用 **dirid**
//! （目录 ID，删除后可能被复用）；`get_detail` 用 **tid**（disstid，唯一）。
//! 「我喜欢」目录 ID 固定为 201。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::models::{Song, SongList};
use crate::pagination::{Page, Paged, PagedView};
use crate::protocol::cgi::CgiRequest;

/// 歌单创建者信息（上游 `SonglistCreator`）。
///
/// 实测（2026-09-29）：`CgiGetDiss` 返回的 `dirinfo.creator` 可解析
/// （musicid/nick/headurl/encrypt_uin）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SonglistCreator {
    /// 用户 musicid。
    #[serde(default)]
    pub musicid: i64,
    /// 昵称。
    #[serde(default)]
    pub nick: String,
    /// 头像地址。
    #[serde(default)]
    pub headurl: String,
    /// 加密 UIN。
    #[serde(default)]
    pub encrypt_uin: String,
}

/// 歌单详情返回的基础元数据（上游 `SonglistInfo`）。
///
/// `list` 以 flatten 复用 [`SongList`] 字段（`dirinfo` 内的
/// `dissid`/`dissname`/`picurl`/`songnum` 等别名均可解析）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SonglistInfo {
    /// 歌单基础信息（继承 `SongList` 字段）。
    #[serde(flatten)]
    pub list: SongList,
    /// 歌单创建者信息。
    #[serde(default)]
    pub creator: SonglistCreator,
}

/// 歌单详情响应（上游 `GetSonglistDetailResponse`）。
///
/// 实测（2026-09-29）：`CgiGetDiss` 的业务结果放在 `data.code`
/// （歌单不存在时为 -100006），`info`（别名 `dirinfo`）/`songlist`/
/// `total_song_num` 均可解析；请求层业务码 0 时才返回本结构。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GetSonglistDetailResponse {
    /// 返回码。
    #[serde(default)]
    pub code: i64,
    /// 子返回码。
    #[serde(default)]
    pub subcode: i64,
    /// 附加消息。
    #[serde(default)]
    pub msg: String,
    /// 歌单基础信息。
    #[serde(default, alias = "dirinfo")]
    pub info: SonglistInfo,
    /// 当前返回的歌曲数量。
    #[serde(default, alias = "songlist_size")]
    pub size: i64,
    /// 当前页歌曲列表。
    #[serde(default, alias = "songlist")]
    pub songs: Vec<Song>,
    /// 歌单歌曲总数。
    #[serde(default, alias = "total_song_num")]
    pub total: i64,
    /// 是否还有更多。
    #[serde(default)]
    pub hasmore: i64,
}

/// 创建/删除歌单响应（上游 `CreateDeleteSonglistResp`）。
///
/// 除顶层 `retCode` 外，tid/dirId/dirName 位于 `$.result` 子对象
/// （由 [`extract_result_fields`] 提取；2026-09-29 实测确认）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CreateDeleteSonglistResp {
    /// 返回码（0=成功）。
    #[serde(default, alias = "retCode")]
    pub ret_code: i64,
    /// 创建成功的歌单 ID（`$.result.tid`，即 disstid）。
    #[serde(default)]
    pub id: i64,
    /// 创建成功的歌单目录 ID（`$.result.dirId`）。
    #[serde(default)]
    pub dirid: i64,
    /// 创建成功的歌单名称（`$.result.dirName`）。
    #[serde(default)]
    pub name: String,
}

/// 提取 `$.result.{tid,dirId,dirName}` 填充写响应（上游 jsonpath；API 与测试共用）。
pub(crate) fn extract_result_fields(data: &Value, resp: &mut CreateDeleteSonglistResp) {
    if let Some(result) = data.get("result") {
        if let Some(v) = result.get("tid").and_then(|v| v.as_i64()) {
            resp.id = v;
        }
        if let Some(v) = result.get("dirId").and_then(|v| v.as_i64()) {
            resp.dirid = v;
        }
        if let Some(v) = result.get("dirName").and_then(|v| v.as_str()) {
            resp.name = v.to_owned();
        }
    }
}

/// 歌单 API（对应上游 `SonglistApi`）。
pub struct SonglistApi<'a> {
    client: &'a QqMusicClient,
}

// ---------------------------------------------------------------------------
// 统一分页视图（has_more 归一规则见 crate::pagination 模块文档）
// ---------------------------------------------------------------------------

/// 显式 hasmore 字段（`hasmore`，0/1），以服务端为准（规则 1）。
///
/// 「我喜欢」（`user.get_fav_song`）复用本响应类型，规则一致。
impl Paged for GetSonglistDetailResponse {
    type Item = Song;

    fn paged(&self, page: Page) -> PagedView<'_, Song> {
        PagedView {
            items: &self.songs,
            total: self.total,
            has_more: self.hasmore != 0,
            page,
        }
    }
}

impl<'a> SonglistApi<'a> {
    /// 构造歌单 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 获取歌单详细信息（上游 `get_detail`；登录：免登录）。
    ///
    /// `songlist_id` 为歌单 disstid/tid（自建歌单用创建响应返回的 tid）；
    /// `dirid` 一般传 0；`page` 为分页窗口；`onlysong`/`tag`/`userinfo`
    /// 对齐上游开关。
    ///
    /// wire 参数归一（上游签名 `num`/`page` 两参数冗余，Rust 统一由
    /// [`Page`] 派生，wire 语义不变）：`song_begin = page.offset()`
    /// （上游 `num * (page - 1)`，即偏移）、`song_num = page.num`
    /// （即页大小）——两个 wire 键服务端均需要，分别承担偏移与页大小
    /// 语义，非冗余。
    ///
    /// 实测（2026-09-29）：可用（含新建空歌单与加/删歌后的状态核对；
    /// 歌单不存在时 `data.code` 为 -100006，`info`/`songs` 为空）。
    pub async fn get_detail(
        &self,
        songlist_id: i64,
        dirid: i64,
        page: Page,
        onlysong: bool,
        tag: bool,
        userinfo: bool,
    ) -> Result<GetSonglistDetailResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.srfDissInfo.DissInfo",
            "CgiGetDiss",
            json!({
                "disstid": songlist_id,
                "dirid": dirid,
                "tag": tag,
                "song_begin": page.offset(),
                "song_num": page.num,
                "userinfo": userinfo,
                "orderlist": true,
                "onlysonglist": onlysong,
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<GetSonglistDetailResponse>(data).map_err(|e| {
            QqMusicError::InvalidResponse(format!("failed to parse songlist detail: {e}"))
        })
    }

    /// 创建歌单（上游 `create`；登录：需登录）。重名不失败，服务端自动加时间戳。
    ///
    /// 返回 `id` 为歌单 tid（disstid，`get_detail`/收藏歌单用），`dirid` 为目录 ID
    /// （`add_songs`/`del_songs`/`delete` 用）。注意 dirid 在删除后可能被新歌单复用
    /// （2026-09-29 实测：删除 dirid=12 后重建仍得 dirid=12），唯一标识用 tid。
    ///
    /// 实测（2026-09-29）：可用（`"HMP-API-TEST-<时间戳>"` 创建成功，retCode=0）。
    pub async fn create(
        &self,
        dirname: &str,
        credential: &Credential,
    ) -> Result<CreateDeleteSonglistResp, QqMusicError> {
        self.write_op("AddPlaylist", json!({"dirName": dirname}), credential)
            .await
    }

    /// 删除歌单（上游 `delete`；登录：需登录）。删除不存在的歌单返回 dirid=0。
    ///
    /// 实测（2026-09-29）：可用（临时歌单删除后 `get_detail` 报 -100006、
    /// 自建歌单列表不再包含该 tid）。
    pub async fn delete(
        &self,
        dirid: i64,
        credential: &Credential,
    ) -> Result<CreateDeleteSonglistResp, QqMusicError> {
        self.write_op("DelPlaylist", json!({"dirId": dirid}), credential)
            .await
    }

    /// 添加歌曲到歌单（上游 `add_songs`；登录：需登录）。
    ///
    /// `dirid` 为歌单目录 ID；`song_info` 每项为 `(song_id, song_type)`
    /// （普通歌曲 songType=0，与上游 `Song.type` 一致）；`tid` 为歌单 tid（可传 0）。
    /// 歌曲已存在于歌单也返回 `true`；上游对 CGI 错误 80092 返回 `false`，
    /// 其余错误原样抛出（本实现以 `allow_error_codes` 对齐）。
    ///
    /// 实测（2026-09-29）：可用。
    pub async fn add_songs(
        &self,
        dirid: i64,
        song_info: &[(i64, i64)],
        tid: i64,
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        self.detail_write("AddSonglist", dirid, song_info, tid, credential)
            .await
    }

    /// 删除歌单中的歌曲（上游 `del_songs`；登录：需登录）。
    ///
    /// 参数同 [`SonglistApi::add_songs`]；歌曲不在歌单中也返回 `true`。
    ///
    /// 实测（2026-09-29）：可用。
    pub async fn del_songs(
        &self,
        dirid: i64,
        song_info: &[(i64, i64)],
        tid: i64,
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        self.detail_write("DelSonglist", dirid, song_info, tid, credential)
            .await
    }

    /// 收藏歌曲到「我喜欢」歌单（上游 `like_song`，固定 dirid=201；登录：需登录）。
    ///
    /// `song_info` 每项为 `(song_id, song_type)`；即以 dirid=201 调用
    /// [`SonglistApi::add_songs`]。歌曲已在「我喜欢」中也返回 `true`。
    ///
    /// 实测（2026-09-29）：请求形态正确（同参数的 unlike 返回 true），但对测试
    /// 歌曲（186016《开始懂了》）服务端返回 CGI 错误 80105（触发条件未查明，
    /// 疑似歌曲权益/类型限制；上游对 80105 亦原样抛错）。
    pub async fn like_song(
        &self,
        song_info: &[(i64, i64)],
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        self.add_songs(201, song_info, 0, credential).await
    }

    /// 从「我喜欢」歌单移除歌曲（上游 `unlike_song`，固定 dirid=201；登录：需登录）。
    ///
    /// `song_info` 每项为 `(song_id, song_type)`；歌曲不在「我喜欢」中也返回
    /// `Ok(true)`（2026-09-29 实测：对未收藏歌曲返回 true）。
    pub async fn unlike_song(
        &self,
        song_info: &[(i64, i64)],
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        self.del_songs(201, song_info, 0, credential).await
    }

    /// 歌单写操作（AddPlaylist/DelPlaylist）统一入口。
    async fn write_op(
        &self,
        method: &str,
        param: Value,
        credential: &Credential,
    ) -> Result<CreateDeleteSonglistResp, QqMusicError> {
        let request = CgiRequest::new("music.musicasset.PlaylistBaseWrite", method, param)
            .with_require_login(true);
        let data = self
            .client
            .musicu_request(&request, Some(credential))
            .await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let mut resp: CreateDeleteSonglistResp =
            serde_json::from_value(data.clone()).map_err(|e| {
                QqMusicError::InvalidResponse(format!("failed to parse playlist write: {e}"))
            })?;
        extract_result_fields(&data, &mut resp);
        Ok(resp)
    }

    /// 歌单歌曲写操作（AddSonglist/DelSonglist）统一入口。
    ///
    /// 上游对 CGI 错误 80092 捕获后返回 `false`（`add_songs`/`del_songs`），
    /// 其余错误原样抛出；本实现以 `allow_error_codes = [80092]` 对齐
    /// （此时内层无 `retCode` → `Ok(false)`）。
    async fn detail_write(
        &self,
        method: &str,
        dirid: i64,
        song_info: &[(i64, i64)],
        tid: i64,
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        let v_song_info: Vec<Value> = song_info
            .iter()
            .map(|(song_id, song_type)| json!({"songId": song_id, "songType": song_type}))
            .collect();
        let mut request = CgiRequest::new(
            "music.musicasset.PlaylistDetailWrite",
            method,
            json!({
                "dirId": dirid,
                "tid": tid,
                "bFmtUtf8": true,
                "v_songInfo": v_song_info,
            }),
        )
        .with_require_login(true);
        request.allow_error_codes = Some(vec![80092]);
        let data = self
            .client
            .musicu_request(&request, Some(credential))
            .await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        match data.get("retCode").and_then(|v| v.as_i64()) {
            Some(0) => Ok(true),
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_songlist_detail_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/songlist/detail.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: GetSonglistDetailResponse = serde_json::from_value(data.clone()).unwrap();

        assert_eq!(resp.code, 0);
        assert!(
            !resp.info.list.title.is_empty(),
            "title should be non-empty"
        );
        assert!(!resp.info.creator.nick.is_empty(), "creator nick");
        assert_eq!(resp.songs.len(), 5);
        assert_eq!(resp.total, 30);
        assert!(resp.hasmore > 0);
        // 歌曲应具备播放身份
        let song = &resp.songs[0];
        assert!(!song.mid.is_empty());
        assert!(!song.name.is_empty());
        assert!(!song.singer.is_empty());
    }

    #[test]
    fn create_resp_parses_result_subobject() {
        let raw = json!({
            "retCode": 0,
            "result": {"tid": 12345, "dirId": 678, "dirName": "我的收藏"}
        });
        let mut resp: CreateDeleteSonglistResp = serde_json::from_value(raw.clone()).unwrap();
        extract_result_fields(&raw, &mut resp);
        assert_eq!(resp.ret_code, 0);
        assert_eq!(resp.id, 12345);
        assert_eq!(resp.dirid, 678);
        assert_eq!(resp.name, "我的收藏");
    }
}
