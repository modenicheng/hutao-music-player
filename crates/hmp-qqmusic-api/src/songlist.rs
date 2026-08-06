//! 歌单模块（对应上游 `modules/songlist.py`）。
//!
//! `get_detail` 免登录；创建/删除/加歌/删歌/收藏均需登录态，
//! 凭证由调用方显式传入 `&Credential`（§6.4 凭证解耦）。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::models::{Song, SongList};
use crate::protocol::cgi::CgiRequest;

/// 歌单创建者信息（上游 `SonglistCreator`）。
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
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CreateDeleteSonglistResp {
    /// 返回码（0=成功）。
    #[serde(default)]
    pub ret_code: i64,
    /// 创建成功的歌单 ID。
    #[serde(default)]
    pub id: i64,
    /// 创建成功的歌单目录 ID。
    #[serde(default)]
    pub dirid: i64,
    /// 创建成功的歌单名称。
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

impl<'a> SonglistApi<'a> {
    /// 构造歌单 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 获取歌单详细信息（上游 `get_detail`）。
    #[allow(clippy::too_many_arguments)] // 对齐上游签名（songlist_id/dirid/num/page/onlysong/tag/userinfo）
    pub async fn get_detail(
        &self,
        songlist_id: i64,
        dirid: i64,
        num: i64,
        page: i64,
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
                "song_begin": num * (page - 1),
                "song_num": num,
                "userinfo": userinfo,
                "orderlist": true,
                "onlysonglist": onlysong,
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<GetSonglistDetailResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("songlist detail 解析失败: {e}")))
    }

    /// 创建歌单（上游 `create`）。重名不失败，服务端自动加时间戳。
    pub async fn create(
        &self,
        dirname: &str,
        credential: &Credential,
    ) -> Result<CreateDeleteSonglistResp, QqMusicError> {
        self.write_op("AddPlaylist", json!({"dirName": dirname}), credential)
            .await
    }

    /// 删除歌单（上游 `delete`）。删除不存在的歌单返回 dirid=0。
    pub async fn delete(
        &self,
        dirid: i64,
        credential: &Credential,
    ) -> Result<CreateDeleteSonglistResp, QqMusicError> {
        self.write_op("DelPlaylist", json!({"dirId": dirid}), credential)
            .await
    }

    /// 添加歌曲到歌单（上游 `add_songs`）。
    ///
    /// 歌曲已存在于歌单也返回 `true`；无权限等返回 `false`。
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

    /// 删除歌单中的歌曲（上游 `del_songs`）。
    ///
    /// 歌曲不在歌单中也返回 `true`。
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

    /// 收藏歌曲到「我喜欢」歌单（上游 `like_song`，固定 dirid=201）。
    pub async fn like_song(
        &self,
        song_info: &[(i64, i64)],
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        self.add_songs(201, song_info, 0, credential).await
    }

    /// 从「我喜欢」歌单移除歌曲（上游 `unlike_song`，固定 dirid=201）。
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
        let mut resp: CreateDeleteSonglistResp = serde_json::from_value(data.clone())
            .map_err(|e| QqMusicError::InvalidResponse(format!("playlist write 解析失败: {e}")))?;
        extract_result_fields(&data, &mut resp);
        Ok(resp)
    }

    /// 歌单歌曲写操作（AddSonglist/DelSonglist）统一入口。
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
        let request = CgiRequest::new(
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
