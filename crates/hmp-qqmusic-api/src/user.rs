//! 用户库 API（对应上游 `UserApi`，模块 `qqmusic_api/modules/user.py`）。
//!
//! 覆盖审计第 4 步需求：自建歌单（`get_created_songlist`）、我喜欢
//! （`get_fav_song`）、收藏歌单（`get_fav_songlist` / `fav_songlist` /
//! `unfav_songlist`）、收藏专辑（`get_fav_album`）、主页与 VIP。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::models::{Album, SongList};
use crate::protocol::cgi::CgiRequest;

/// 自建歌单响应（上游 `UserCreatedSonglistResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UserCreatedSonglistResponse {
    /// 歌单列表。
    #[serde(default, alias = "vecSonglist", alias = "songlist")]
    pub songlist: Vec<SongList>,
    /// 总数。
    #[serde(default)]
    pub total: i64,
}

/// 收藏歌单响应（上游 `UserFavSonglistResponse`）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UserFavSonglistResponse {
    /// 歌单列表。
    #[serde(default, alias = "vecSonglist")]
    pub playlists: Vec<SongList>,
    /// 总数。
    #[serde(default)]
    pub total: i64,
    /// 是否还有更多页。
    #[serde(default)]
    pub hasmore: i64,
}

/// 收藏专辑响应（上游 `UserFavAlbumResponse`；元素即专辑模型）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UserFavAlbumResponse {
    /// 专辑列表。
    #[serde(default, alias = "vecAlbum")]
    pub albums: Vec<Album>,
    /// 总数。
    #[serde(default)]
    pub total: i64,
    /// 是否还有更多页。
    #[serde(default)]
    pub hasmore: i64,
}

/// 用户库 API。
pub struct UserApi<'a> {
    client: &'a QqMusicClient,
}

impl<'a> UserApi<'a> {
    /// 构造用户库 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 用户创建的歌单列表（上游 `get_created_songlist`）。
    pub async fn get_created_songlist(
        &self,
        uin: &str,
        credential: Option<&Credential>,
    ) -> Result<UserCreatedSonglistResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.musicasset.PlaylistBaseRead",
            "GetPlaylistByUin",
            json!({ "uin": uin }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("created songlist 解析失败: {e}")))
    }

    /// 「我喜欢」歌曲（上游 `get_fav_song`，dirid=201；返回完整歌曲原始数据）。
    pub async fn get_fav_song(
        &self,
        euin: &str,
        page: i64,
        num: i64,
        credential: Option<&Credential>,
    ) -> Result<crate::songlist::GetSonglistDetailResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.srfDissInfo.DissInfo",
            "CgiGetDiss",
            json!({
                "disstid": 0,
                "dirid": 201,
                "tag": true,
                "song_begin": num * (page - 1),
                "song_num": num,
                "userinfo": true,
                "orderlist": true,
                "enc_host_uin": euin,
            }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("fav song 解析失败: {e}")))
    }

    /// 收藏的歌单列表（上游 `get_fav_songlist`）。
    pub async fn get_fav_songlist(
        &self,
        euin: &str,
        page: i64,
        num: i64,
        credential: Option<&Credential>,
    ) -> Result<UserFavSonglistResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.musicasset.PlaylistFavRead",
            "CgiGetPlaylistFavInfo",
            json!({ "uin": euin, "offset": (page - 1) * num, "size": num }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("fav songlist 解析失败: {e}")))
    }

    /// 收藏的专辑列表（上游 `get_fav_album`）。
    pub async fn get_fav_album(
        &self,
        euin: &str,
        page: i64,
        num: i64,
        credential: Option<&Credential>,
    ) -> Result<UserFavAlbumResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.musicasset.AlbumFavRead",
            "CgiGetAlbumFavInfo",
            json!({ "euin": euin, "offset": (page - 1) * num, "size": num }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("fav album 解析失败: {e}")))
    }

    /// 收藏歌单（上游 `fav_songlist`；已在收藏中也返回 true）。
    pub async fn fav_songlist(
        &self,
        songlist_id: i64,
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        self.fav_write("FavPlaylist", songlist_id, credential).await
    }

    /// 取消收藏歌单（上游 `unfav_songlist`；本就未收藏也返回 true）。
    pub async fn unfav_songlist(
        &self,
        songlist_id: i64,
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        self.fav_write("CancelFavPlaylist", songlist_id, credential)
            .await
    }

    /// 收藏/取消收藏歌单统一入口。
    async fn fav_write(
        &self,
        method: &str,
        songlist_id: i64,
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        let request = CgiRequest::new(
            "music.musicasset.PlaylistFavWrite",
            method,
            json!({
                "uin": credential.encrypt_uin,
                "v_playlistId": [songlist_id],
            }),
        )
        .with_require_login(true);
        let data = self
            .client
            .musicu_request(&request, Some(credential))
            .await?;
        let failed: Vec<i64> = data
            .get("v_failedPlaylistId")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        Ok(
            data.get("result").and_then(|v| v.as_i64()) == Some(0)
                && !failed.contains(&songlist_id),
        )
    }

    /// 用户主页头部（上游 `get_homepage`；展示型——保留原始数据供 CLI 提取）。
    pub async fn get_homepage(
        &self,
        euin: &str,
        credential: Option<&Credential>,
    ) -> Result<Value, QqMusicError> {
        let request = CgiRequest::new(
            "music.UnifiedHomepage.UnifiedHomepageSrv",
            "GetHomepageHeader",
            json!({ "uin": euin, "IsQueryTabDetail": 1 }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        Ok(data.get("data").cloned().unwrap_or(json!({})))
    }

    /// 当前账号 VIP 信息（上游 `get_vip_info`；展示型——保留原始数据）。
    pub async fn get_vip_info(&self, credential: &Credential) -> Result<Value, QqMusicError> {
        let request = CgiRequest::new("VipLogin.VipLoginInter", "vip_login_base", json!({}))
            .with_require_login(true);
        let data = self
            .client
            .musicu_request(&request, Some(credential))
            .await?;
        Ok(data.get("data").cloned().unwrap_or(json!({})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::Credential;

    /// 凭证（测试用，不触网）。
    fn cred() -> Credential {
        Credential {
            uin: "1".into(),
            music_id: "1".into(),
            music_key: "k".into(),
            encrypt_uin: "00000000000000000000000000000000".into(),
            ..Default::default()
        }
    }

    #[test]
    fn fav_songlist_write_parses_success() {
        // fav_write 的解析逻辑：result=0 且无 failed → true。
        let parsed = fav_write_parse_for_test(json!({"result": 0, "v_failedPlaylistId": []}), 42);
        assert!(parsed);
        let failed = fav_write_parse_for_test(json!({"result": 0, "v_failedPlaylistId": [42]}), 42);
        assert!(!failed, "目标歌单出现在 failed 列表 → false");
    }

    /// 复制 fav_write 的判定逻辑（响应解析为纯函数便于测试）。
    fn fav_write_parse_for_test(data: Value, songlist_id: i64) -> bool {
        let failed: Vec<i64> = data
            .get("v_failedPlaylistId")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        data.get("result").and_then(|v| v.as_i64()) == Some(0) && !failed.contains(&songlist_id)
    }

    #[test]
    fn created_songlist_defaults_on_missing_fields() {
        let v: UserCreatedSonglistResponse =
            serde_json::from_value(json!({})).expect("宽松反序列化：缺失字段走 default");
        assert!(v.songlist.is_empty());
        let v2: UserCreatedSonglistResponse = serde_json::from_value(json!({
            "vecSonglist": [{"dissname": "我的歌单", "dissid": 123}],
            "total": 1,
        }))
        .expect("alias vecSonglist 应识别");
        assert_eq!(v2.songlist.len(), 1);
        assert_eq!(v2.songlist[0].id, 123);
        assert_eq!(v2.songlist[0].title, "我的歌单");
    }

    #[test]
    fn fav_songlist_defaults_on_missing_fields() {
        let v: UserFavSonglistResponse = serde_json::from_value(json!({
            "vecSonglist": [{"dissname": "华语精选"}],
        }))
        .expect("alias vecSonglist 应识别");
        assert_eq!(v.playlists.len(), 1);
        assert_eq!(v.playlists[0].title, "华语精选");
    }

    #[test]
    fn cred_has_encrypt_uin() {
        let c = cred();
        assert!(!c.encrypt_uin.is_empty());
    }
}
