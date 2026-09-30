//! 用户库 API（对应上游 `UserApi`，模块 `qqmusic_api/modules/user.py`）。
//!
//! 覆盖审计第 4 步需求：自建歌单（`get_created_songlist`）、我喜欢
//! （`get_fav_song`）、收藏歌单（`get_fav_songlist` / `fav_songlist` /
//! `unfav_songlist`）、收藏专辑（`get_fav_album`）、主页与 VIP、
//! 音乐基因（`get_music_gene`，上游缺口补齐）。
//!
//! uin 参数速查：`get_created_songlist` 用**数字 uin**（传加密 uin → 80030）；
//! 其余 `euin` 参数均用**加密 uin**（`Credential::encrypt_uin`）。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::models::{Album, SongList};
use crate::pagination::{Page, Paged, PagedView};
use crate::protocol::cgi::CgiRequest;

/// 自建歌单响应（上游 `UserCreatedSonglistResponse`）。
///
/// 实测（2026-09-29）：`GetPlaylistByUin` 返回 `total`/`v_playlist`/`bFinish`
/// 可全部解析（total=10 → 10 条）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UserCreatedSonglistResponse {
    /// 歌单列表（上游 jsonpath `$.v_playlist[*]`，元素兼容 [`SongList`] 别名）。
    #[serde(
        default,
        alias = "vecSonglist",
        alias = "songlist",
        alias = "v_playlist"
    )]
    pub songlist: Vec<SongList>,
    /// 总数。
    #[serde(default)]
    pub total: i64,
    /// 上游返回的已删除歌单 ID 标记（上游 `deleted_ids`）。
    #[serde(default, alias = "v_delTid")]
    pub deleted_ids: Vec<i64>,
    /// 是否已拉取完成（上游 `finished`）。
    #[serde(default, alias = "bFinish")]
    pub finished: bool,
}

/// 收藏歌单响应（上游 `UserFavSonglistResponse`）。
///
/// 实测（2026-09-29）：`PlaylistFavRead/CgiGetPlaylistFavInfo` 的 `data` 键为
/// `number`/`hasmore`/`v_list`/`v_failTids`/`v_delTids`/`total`/`hide`；
/// 列表元素键为 `tid`/`dirId`/`name`/`songnum`/`logo`/`createtime`/`orderTime` 等。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UserFavSonglistResponse {
    /// 当前页请求/返回数量（上游 `number`）。
    #[serde(default)]
    pub number: i64,
    /// 歌单列表（线上数据键 `v_list`；`vecSonglist` 为早期兼容别名）。
    #[serde(default, alias = "vecSonglist", alias = "v_list")]
    pub playlists: Vec<SongList>,
    /// 收藏歌单总数。
    #[serde(default)]
    pub total: i64,
    /// 是否还有更多页（0/1）。
    #[serde(default)]
    pub hasmore: i64,
    /// 上游返回的已删除歌单 ID 列表（上游 `deleted_ids`）。
    #[serde(default, alias = "v_delTids")]
    pub deleted_ids: Vec<i64>,
    /// 拉取失败的歌单 ID 列表（上游 `failed_ids`）。
    #[serde(default, alias = "v_failTids")]
    pub failed_ids: Vec<i64>,
}

/// 收藏专辑响应（上游 `UserFavAlbumResponse`；元素即专辑模型）。
///
/// 实测（2026-09-29）：`AlbumFavRead/CgiGetAlbumFavInfo` 的 `data` 键为
/// `number`/`hasmore`/`v_list`/`v_failAlbumId`/`total`/`hide`。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UserFavAlbumResponse {
    /// 当前页请求/返回数量（上游 `number`）。
    #[serde(default)]
    pub number: i64,
    /// 专辑列表（线上数据键 `v_list`；`vecAlbum` 为早期兼容别名）。
    #[serde(default, alias = "vecAlbum", alias = "v_list")]
    pub albums: Vec<Album>,
    /// 收藏专辑总数。
    #[serde(default)]
    pub total: i64,
    /// 是否还有更多页（0/1）。
    #[serde(default)]
    pub hasmore: i64,
    /// 拉取失败的专辑 ID 列表（上游 `failed_album_ids`）。
    #[serde(default, alias = "v_failAlbumId")]
    pub failed_album_ids: Vec<i64>,
}

/// 音乐基因响应（上游 `UserMusicGeneResponse`；展示型——名片强类型，其余保留原文）。
///
/// 实测（2026-09-29）：`GetProfileReport` 可用，`UserInfoCard.NickName`/
/// `Signature` 正常返回。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UserMusicGeneResponse {
    /// 用户名片（昵称/头像/签名）。
    #[serde(default, alias = "UserInfoCard")]
    pub userinfo_card: UserInfoCard,
    /// 其余音乐基因数据（听歌月报/流派榜/人格卡片等，展示型保留原文）。
    #[serde(flatten)]
    pub extra: Value,
}

/// 用户名片（上游 `UserInfoCard`；音乐基因响应头部的昵称/头像/签名块）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UserInfoCard {
    /// 昵称。
    #[serde(default, alias = "NickName")]
    pub nick_name: String,
    /// 头像地址。
    #[serde(default, alias = "HeadUrl")]
    pub head_url: String,
    /// 个性签名。
    #[serde(default, alias = "Signature")]
    pub signature: String,
}

/// NodeToken（官方网页端 `Date.now().toString()`：当前毫秒时间戳字符串）。
///
/// 服务端要求该参数存在（值任意）；`get_homepage` 缺少它时返回业务错误码 10000。
fn node_token() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 统一分页视图（has_more 归一规则见 crate::pagination 模块文档）
// ---------------------------------------------------------------------------

/// 显式 hasmore 字段（`hasmore`，0/1），以服务端为准（规则 1）。
impl Paged for UserFavSonglistResponse {
    type Item = SongList;

    fn paged(&self, page: Page) -> PagedView<'_, SongList> {
        PagedView {
            items: &self.playlists,
            total: self.total,
            has_more: self.hasmore != 0,
            page,
        }
    }
}

/// 显式 hasmore 字段（`hasmore`，0/1），以服务端为准（规则 1）。
impl Paged for UserFavAlbumResponse {
    type Item = Album;

    fn paged(&self, page: Page) -> PagedView<'_, Album> {
        PagedView {
            items: &self.albums,
            total: self.total,
            has_more: self.hasmore != 0,
            page,
        }
    }
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

    /// 用户创建的歌单列表（上游 `get_created_songlist`；登录：登录增强，免登录可用）。
    ///
    /// `uin` 必须是**数字 UIN**（如 `"939861972"`，即 `Credential::uin`）；
    /// 传加密 uin 服务端返回业务错误码 80030（2026-09-29 实测）。
    ///
    /// 实测（2026-09-29）：可用（total=10 解析 10 条，`finished`=true）。
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
        serde_json::from_value(data).map_err(|e| {
            QqMusicError::InvalidResponse(format!("failed to parse created songlist: {e}"))
        })
    }

    /// 「我喜欢」歌曲列表（上游 `get_fav_song`，dirid=201；登录：登录增强，免登录可用）。
    ///
    /// `euin` 必须是**加密 UIN**（`Credential::encrypt_uin`，作为 `enc_host_uin`
    /// 参数传给 `CgiGetDiss`）；`page` 为分页窗口（服务端
    /// `song_begin = page.offset()`、`song_num = page.num`）。返回完整歌曲
    /// 原始数据（复用 [`crate::songlist::GetSonglistDetailResponse`]）。
    ///
    /// 实测（2026-09-29）：可用（total=801，按 num=100 分页正常返回）。
    pub async fn get_fav_song(
        &self,
        euin: &str,
        page: Page,
        credential: Option<&Credential>,
    ) -> Result<crate::songlist::GetSonglistDetailResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.srfDissInfo.DissInfo",
            "CgiGetDiss",
            json!({
                "disstid": 0,
                "dirid": 201,
                "tag": true,
                "song_begin": page.offset(),
                "song_num": page.num,
                "userinfo": true,
                "orderlist": true,
                "enc_host_uin": euin,
            }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("failed to parse fav song: {e}")))
    }

    /// 收藏的外部歌单列表（上游 `get_fav_songlist`；登录：登录增强，免登录可用）。
    ///
    /// `euin` 必须是**加密 UIN**，参数键为 `uin`（注意与 [`UserApi::get_fav_album`]
    /// 的 `euin` 键名不同，2026-09-29 实测确认）；`page` 为分页窗口（服务端
    /// `offset = page.offset()`、`size = page.num`）。
    ///
    /// 实测（2026-09-29）：可用（total=12，`hasmore`=1；列表数据键为
    /// `v_list`，元素含 `tid`/`dirId`/`name`/`logo` 等）。
    pub async fn get_fav_songlist(
        &self,
        euin: &str,
        page: Page,
        credential: Option<&Credential>,
    ) -> Result<UserFavSonglistResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.musicasset.PlaylistFavRead",
            "CgiGetPlaylistFavInfo",
            json!({ "uin": euin, "offset": page.offset(), "size": page.num }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data).map_err(|e| {
            QqMusicError::InvalidResponse(format!("failed to parse fav songlist: {e}"))
        })
    }

    /// 收藏的专辑列表（上游 `get_fav_album`；登录：登录增强，免登录可用）。
    ///
    /// `euin` 必须是**加密 UIN**，参数键为 `euin`（注意与 [`UserApi::get_fav_songlist`]
    /// 的 `uin` 键名不同，2026-09-29 实测确认）；`page` 为分页窗口（服务端
    /// `offset = page.offset()`、`size = page.num`；实测单页 20 条时服务端
    /// 分页行为正常）。
    ///
    /// 实测（2026-09-29）：可用（total=32，`hasmore`=1；列表数据键为 `v_list`）。
    pub async fn get_fav_album(
        &self,
        euin: &str,
        page: Page,
        credential: Option<&Credential>,
    ) -> Result<UserFavAlbumResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.musicasset.AlbumFavRead",
            "CgiGetAlbumFavInfo",
            json!({ "euin": euin, "offset": page.offset(), "size": page.num }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("failed to parse fav album: {e}")))
    }

    /// 收藏歌单（上游 `fav_songlist`；登录：需登录）。
    ///
    /// `songlist_id` 为歌单 disstid/tid（**不是**自建歌单的 dirid）。已在收藏中
    /// 也返回 `true`（上游语义）。
    ///
    /// 服务端限制（2026-09-29 实测）：不能收藏自己创建的歌单和「我喜欢」目录，
    /// 此时服务端返回 `result=80184`（reason `"can't order self's dir or 201 dir"`，
    /// `v_failedPlaylistId` 为空）→ `Ok(false)`。他人公开歌单的成功路径未做真机
    /// 验证（避免改动用户真实收藏），wire 语义由离线回归覆盖。
    pub async fn fav_songlist(
        &self,
        songlist_id: i64,
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        self.fav_write("FavPlaylist", songlist_id, credential).await
    }

    /// 取消收藏歌单（上游 `unfav_songlist`；登录：需登录）。
    ///
    /// `songlist_id` 为歌单 disstid/tid；本就未收藏也返回 `Ok(true)`
    /// （服务端 `result=0`，2026-09-29 实测）。
    pub async fn unfav_songlist(
        &self,
        songlist_id: i64,
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        self.fav_write("CancelFavPlaylist", songlist_id, credential)
            .await
    }

    /// 收藏写操作的响应判定（上游 `fav_songlist`/`unfav_songlist` 返回值逻辑）。
    ///
    /// 上游 `_build_cgi`（无 response_model）返回**内层 `data` 对象**，再取
    /// `result == 0` 且目标不在 `v_failedPlaylistId` 中；`musicu_request` 返回
    /// 子响应 `{code, data}`，故须先解包内层（2026-09-29 修复的回归点）。
    fn fav_write_result(data: &Value, songlist_id: i64) -> bool {
        let inner = data.get("data").unwrap_or(&Value::Null);
        let failed: Vec<i64> = inner
            .get("v_failedPlaylistId")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        inner.get("result").and_then(|v| v.as_i64()) == Some(0) && !failed.contains(&songlist_id)
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
        Ok(Self::fav_write_result(&data, songlist_id))
    }

    /// 用户主页头部（上游 `get_homepage`；登录：登录增强，免登录可用；
    /// 展示型——保留原始数据供 CLI 提取）。
    ///
    /// 与上游差异：额外携带 `NodeToken`（当前毫秒时间戳字符串）。
    /// 上游请求缺省该参数时，服务端返回业务错误码 10000 且 `data` 为空壳；
    /// 官方网页端（share/profile_v2）亦发送 `NodeToken: Date.now().toString()`。
    ///
    /// # 服务端状态（2026-09-29 实测）
    ///
    /// 该 CGI 目前**对合法参数也整体返回 10000 空壳**（`Info` 全空、
    /// `EncryptedUin`/`Name` 为空字符串）：加密 uin、数字 uin、有无
    /// `NodeToken` 均一致；参数校验仍在运行（数字型 `NodeToken` 触发 10006）。
    /// 上游 Python 参考实现同样受影响。取昵称/头像请改用
    /// [`UserApi::get_music_gene`]（`GetProfileReport`，同日实测可用）。
    /// 该 CGI 历史上另有按 IP 的滚动限流（约 20+ 次后一律 10000，静置恢复）。
    pub async fn get_homepage(
        &self,
        euin: &str,
        credential: Option<&Credential>,
    ) -> Result<Value, QqMusicError> {
        let request = CgiRequest::new(
            "music.UnifiedHomepage.UnifiedHomepageSrv",
            "GetHomepageHeader",
            json!({
                "uin": euin,
                "IsQueryTabDetail": 1,
                "NodeToken": node_token(),
            }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        Ok(data.get("data").cloned().unwrap_or(json!({})))
    }

    /// 当前账号 VIP 信息（上游 `get_vip_info`；登录：需登录；展示型——保留原始数据）。
    ///
    /// 实测（2026-09-29）：可用（返回 `identity`/`userinfo`/`maxdirnum`/
    /// `maxsongnum`/`svip` 等 40+ 展示字段）。
    pub async fn get_vip_info(&self, credential: &Credential) -> Result<Value, QqMusicError> {
        let request = CgiRequest::new("VipLogin.VipLoginInter", "vip_login_base", json!({}))
            .with_require_login(true);
        let data = self
            .client
            .musicu_request(&request, Some(credential))
            .await?;
        Ok(data.get("data").cloned().unwrap_or(json!({})))
    }

    /// 用户音乐基因（上游 `get_music_gene`；登录：登录增强，免登录可用；
    /// 展示型——用户名片 + 听歌画像）。
    ///
    /// `euin` 为加密 uin（`VisitAccount` 参数）。响应中 [`UserMusicGeneResponse::userinfo_card`]
    /// 携带昵称/头像/签名，是 [`UserApi::get_homepage`] 服务端异常（2026-09-29 起
    /// 返回 10000 空壳）时获取账号昵称的可靠来源；其余字段（流派榜/人格/
    /// 听歌月报等）为展示型原始数据。
    ///
    /// 实测（2026-09-29）：可用（昵称与签名正常返回）。
    pub async fn get_music_gene(
        &self,
        euin: &str,
        credential: Option<&Credential>,
    ) -> Result<UserMusicGeneResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.recommend.UserProfileSettingSvr",
            "GetProfileReport",
            json!({ "VisitAccount": euin }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("failed to parse music gene: {e}")))
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
    fn fav_songlist_write_parses_inner_data() {
        // 上游 _build_cgi（无 response_model）返回内层 data 对象的判定逻辑：
        // result=0 且无 failed → true（回归：必须解包内层 data）。
        assert!(UserApi::fav_write_result(
            &json!({"code": 0, "data": {"result": 0, "v_failedPlaylistId": []}}),
            42
        ));
        // 服务端拒绝（自建歌单/201 目录，result=80184）→ false。
        assert!(!UserApi::fav_write_result(
            &json!({
                "code": 0,
                "data": {
                    "reason": "can't order self's dir or 201 dir",
                    "result": 80184,
                    "v_failedPlaylistId": []
                }
            }),
            42
        ));
        // 目标歌单出现在 failed 列表 → false。
        assert!(!UserApi::fav_write_result(
            &json!({"code": 0, "data": {"result": 0, "v_failedPlaylistId": [42]}}),
            42
        ));
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
    fn fav_songlist_parses_wire_v_list() {
        // 2026-09-29 实测 wire 键：v_list/v_failTids/v_delTids/total/hasmore/number。
        let v: UserFavSonglistResponse = serde_json::from_value(json!({
            "number": 10,
            "hasmore": 1,
            "v_list": [{"tid": 111, "dirId": 3, "name": "华语精选", "songnum": 30, "logo": "p"}],
            "total": 12,
            "v_delTids": [1],
            "v_failTids": [2],
            "hide": false,
        }))
        .expect("v_list 别名应识别");
        assert_eq!(v.playlists.len(), 1);
        assert_eq!(v.playlists[0].id, 111);
        assert_eq!(v.playlists[0].dirid, 3);
        assert_eq!(v.total, 12);
        assert_eq!(v.hasmore, 1);
        assert_eq!(v.number, 10);
        assert_eq!(v.deleted_ids, vec![1]);
        assert_eq!(v.failed_ids, vec![2]);
        // 兼容别名 vecSonglist 仍可用
        let v2: UserFavSonglistResponse =
            serde_json::from_value(json!({"vecSonglist": [{"dissname": "x"}]}))
                .expect("vecSonglist 别名应识别");
        assert_eq!(v2.playlists.len(), 1);
    }

    #[test]
    fn fav_album_parses_wire_v_list() {
        // 2026-09-29 实测 wire 键：v_list/v_failAlbumId/total/hasmore/number。
        let v: UserFavAlbumResponse = serde_json::from_value(json!({
            "number": 10,
            "hasmore": 1,
            "v_list": [{"albumID": 15995, "albumMid": "004YCGQa3qOG8u", "albumName": "经典全纪录"}],
            "total": 32,
            "v_failAlbumId": [7],
        }))
        .expect("v_list 别名应识别");
        assert_eq!(v.albums.len(), 1);
        assert_eq!(v.albums[0].id, 15995);
        assert_eq!(v.albums[0].mid, "004YCGQa3qOG8u");
        assert_eq!(v.albums[0].name, "经典全纪录");
        assert_eq!(v.total, 32);
        assert_eq!(v.failed_album_ids, vec![7]);
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
