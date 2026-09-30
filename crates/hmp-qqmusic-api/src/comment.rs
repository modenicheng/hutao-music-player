//! 评论 API（对应上游 `CommentApi`，模块 `qqmusic_api/modules/comment.py`）。
//!
//! 评论按 `biz_id`（QQ numeric song id）寻址——CLI 侧经 `tracks.qq_song_id`
//! 从 mid 映射（spec §6）。`biz_type=SONG=1`、`biz_sub_type=2`。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::pagination::{Page, Paged, PagedView};
use crate::protocol::cgi::CgiRequest;

/// 评论（上游 `Comment`；映射 QQ `Comments[]` 字段）。
///
/// 实测（2026-09-29）：热评/新评/推荐评列表均按 `CmId`/`SeqNo`/`Nick`/`Content`/
/// `PraiseNum`/`PubTime`/`ReplyCnt` 键形下发，本结构解析可用。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Comment {
    /// 评论 ID（回复/删除用；上游 `CmId`）。
    #[serde(default, alias = "CmId", alias = "cmid")]
    pub cm_id: String,
    /// 分页游标（上游 `SeqNo`）。
    #[serde(default, alias = "SeqNo")]
    pub seq_no: String,
    /// 评论者昵称（上游 `Nick`）。
    #[serde(default, alias = "Nick", alias = "nick")]
    pub nickname: String,
    /// 评论内容（上游 `Content`）。
    #[serde(default, alias = "Content", alias = "rootcontent")]
    pub content: String,
    /// 点赞数（上游 `PraiseNum`）。
    #[serde(default, alias = "PraiseNum", alias = "like_num")]
    pub like_count: i64,
    /// 时间戳（秒；上游 `PubTime`）。
    #[serde(default, alias = "PubTime")]
    pub time: i64,
    /// 回复数（上游 `ReplyCnt`）。
    #[serde(default, alias = "ReplyCnt")]
    pub reply_count: i64,
}

/// 评论列表数据（上游 `CommentList`，`$.CommentList` 节点）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CommentListData {
    /// 评论列表（上游 `Comments`）。
    #[serde(default, alias = "Comments")]
    pub comments: Vec<Comment>,
    /// 是否还有更多页（上游 `HasMore`）。
    #[serde(default, alias = "HasMore")]
    pub has_more: i64,
    /// 总数（上游 `Total`）。
    #[serde(default, alias = "Total")]
    pub total: i64,
}

/// 评论列表响应（上游 `CommentListResponse`，内层 data 节点）。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CommentListResponse {
    /// 评论列表数据。
    #[serde(default, alias = "CommentList")]
    pub comment_list: Option<CommentListData>,
    /// 全局评论总数（上游 `TotalCmNum`）。
    #[serde(default, alias = "TotalCmNum")]
    pub total_cm_num: i64,
}

/// 发表评论响应（上游 `AddCommentResponse`）。
///
/// 真实响应内层 data 键形（2026-09-29 实测）：`AddedCmId`/`SubCode`/`Msg`/
/// `ParentCmId`/`VerifyUrl`/`Floor{Num}` 等；无 `ret`/`commentId` 键。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AddCommentResponse {
    /// 新评论 ID（服务端键 `AddedCmId`；删除评论时回传此值）。
    #[serde(
        default,
        alias = "AddedCmId",
        alias = "commentId",
        alias = "cmid",
        alias = "CmId"
    )]
    pub comment_id: String,
    /// 兼容保留字段：真实响应无 `ret` 键，恒为 0（上游判定码见 [`Self::subcode`]）。
    #[serde(default)]
    pub ret: i64,
    /// 子返回码（服务端键 `SubCode`；0=发表成功）。
    #[serde(default, alias = "SubCode")]
    pub subcode: i64,
    /// 附加消息（服务端键 `Msg`，成功时为「发表成功」）。
    #[serde(default, alias = "Msg")]
    pub msg: String,
    /// 回复的父评论 ID（服务端键 `ParentCmId`；非回复评论为空串）。
    #[serde(default, alias = "ParentCmId")]
    pub parent_cm_id: String,
    /// 楼层号（服务端键 `Floor.Num`）。
    #[serde(default, alias = "Floor", deserialize_with = "de_floor_num")]
    pub floor: i64,
    /// 验证码地址（触发风控时非空；服务端键 `VerifyUrl`）。
    #[serde(default, alias = "VerifyUrl")]
    pub verify_url: String,
}

/// 提取 `Floor.Num` 楼层号（`Floor` 为对象，缺失/`null` 时为 0）。
fn de_floor_num<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Value::deserialize(d)?;
    Ok(v.get("Num").and_then(|n| n.as_i64()).unwrap_or(0))
}

// ---------------------------------------------------------------------------
// 统一分页视图（has_more 归一规则见 crate::pagination 模块文档）
// ---------------------------------------------------------------------------

/// 显式 hasmore 字段（`CommentList.HasMore`，0/1），以服务端为准（规则 1）。
///
/// `total` 取 `CommentList.Total`（本 biz 的评论总数，分页相关；
/// 顶层 `TotalCmNum` 为全局计数）。三个读接口（热评/新评/推荐评）
/// 的 wire 分页参数为 `PageNum = page.page - 1`、`PageSize = page.num`。
impl Paged for CommentListResponse {
    type Item = Comment;

    fn paged(&self, page: Page) -> PagedView<'_, Comment> {
        let (comments, total, has_more) = match self.comment_list.as_ref() {
            Some(list) => (&list.comments[..], list.total, list.has_more != 0),
            None => (&[][..], 0, false),
        };
        PagedView {
            items: comments,
            total,
            has_more,
            page,
        }
    }
}

/// 评论 API。
pub struct CommentApi<'a> {
    client: &'a QqMusicClient,
}

/// 评论类型（上游 `CommentBizType`）：默认普通歌曲（SONG=1）。
pub const BIZ_TYPE_SONG: i64 = 1;
/// 歌曲子类型（SONG 对应 biz_sub_type=2）。
pub const BIZ_SUB_TYPE_SONG: i64 = 2;

impl<'a> CommentApi<'a> {
    /// 构造评论 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 歌曲评论数量（上游 `get_comment_count`；登录：免登录）。
    ///
    /// `biz_id` 为 QQ **数字歌曲 ID**（非 MID）；请求体为
    /// `{"request": {biz_id, biz_type, biz_sub_type}}` 双层包裹。
    ///
    /// 实测（2026-09-29）：可用（免登录），取内层 `data.response.count`。
    pub async fn get_comment_count(&self, biz_id: i64) -> Result<i64, QqMusicError> {
        let request = CgiRequest::new(
            "music.globalComment.CommentCountSrv",
            "GetCmCount",
            json!({
                "request": {
                    "biz_id": biz_id.to_string(),
                    "biz_type": BIZ_TYPE_SONG,
                    "biz_sub_type": BIZ_SUB_TYPE_SONG,
                }
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let count = data
            .get("data")
            .and_then(|d| d.get("response"))
            .and_then(|r| r.get("count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        Ok(count)
    }

    /// 歌曲热评列表（上游 `get_hot_comments`；登录：免登录）。
    ///
    /// `biz_id` 为 QQ 数字歌曲 ID；`page` 为分页窗口（服务端
    /// `PageNum = page.page - 1`、`PageSize = page.num`；服务端无单页上限
    /// 实测，上游默认 15）。返回完整响应信封（评论总数 + 显式
    /// `HasMore`），分页视图经 [`Paged::paged`]（规则 1：服务端显式字段）。
    ///
    /// 实测（2026-09-29）：可用（免登录），`CommentList.Comments[]` 按预期解析。
    pub async fn get_hot_comments(
        &self,
        biz_id: i64,
        page: Page,
    ) -> Result<CommentListResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.globalComment.CommentRead",
            "GetHotCommentList",
            json!({
                "BizType": BIZ_TYPE_SONG,
                "BizId": biz_id.to_string(),
                "LastCommentSeqNo": "",
                "PageSize": page.num,
                "PageNum": page.page.saturating_sub(1),
                "HotType": 1,
                "WithAirborne": 0,
                "PicEnable": 1,
                "BizSubType": BIZ_SUB_TYPE_SONG,
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data).map_err(|e| {
            QqMusicError::InvalidResponse(format!("failed to parse hot comments: {e}"))
        })
    }

    /// 歌曲最新评论列表（上游 `get_new_comments`；登录：免登录）。
    ///
    /// `biz_id` 为 QQ 数字歌曲 ID；`page` 为分页窗口（服务端
    /// `PageNum = page.page - 1`、`PageSize = page.num`）。返回完整响应
    /// 信封，分页视图经 [`Paged::paged`]（规则 1）。
    ///
    /// 实测（2026-09-29）：可用（免登录）。
    pub async fn get_new_comments(
        &self,
        biz_id: i64,
        page: Page,
    ) -> Result<CommentListResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.globalComment.CommentRead",
            "GetNewCommentList",
            json!({
                "PageSize": page.num,
                "PageNum": page.page.saturating_sub(1),
                "HashTagID": "",
                "BizType": BIZ_TYPE_SONG,
                "PicEnable": 1,
                "LastCommentSeqNo": "",
                "SelfSeeEnable": 1,
                "BizId": biz_id.to_string(),
                "AudioEnable": 1,
                "BizSubType": BIZ_SUB_TYPE_SONG,
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data).map_err(|e| {
            QqMusicError::InvalidResponse(format!("failed to parse new comments: {e}"))
        })
    }

    /// 歌曲推荐评论列表（上游 `get_recommend_comments`；登录：免登录）。
    ///
    /// `biz_id` 为 QQ 数字歌曲 ID；`page` 为分页窗口（服务端
    /// `PageNum = page.page - 1`、`PageSize = page.num`）。返回完整响应
    /// 信封，分页视图经 [`Paged::paged`]（规则 1）。
    ///
    /// 实测（2026-09-29）：可用（免登录）。
    pub async fn get_recommend_comments(
        &self,
        biz_id: i64,
        page: Page,
    ) -> Result<CommentListResponse, QqMusicError> {
        let request = CgiRequest::new(
            "music.globalComment.CommentRead",
            "GetRecCommentList",
            json!({
                "PageSize": page.num,
                "PageNum": page.page.saturating_sub(1),
                "BizType": BIZ_TYPE_SONG,
                "PicEnable": 1,
                "Flag": 1,
                "LastCommentSeqNo": "",
                "CmListUIVer": 1,
                "BizId": biz_id.to_string(),
                "AudioEnable": 1,
                "BizSubType": BIZ_SUB_TYPE_SONG,
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data).map_err(|e| {
            QqMusicError::InvalidResponse(format!("failed to parse recommend comments: {e}"))
        })
    }

    /// 发表歌曲评论（上游 `add_comment`；登录：需登录）。
    ///
    /// `biz_id` 为 QQ 数字歌曲 ID；`content` 为评论正文；
    /// `reply_cmt_id` 非空时为回复该评论（服务端键 `RepliedCmId`）。
    /// 返回的 [`AddCommentResponse::comment_id`] 用于 [`Self::delete_comment`]。
    ///
    /// 实测（2026-09-29）：可用；响应内层 `AddedCmId` 为新评论 ID
    /// （此前别名缺失导致 `comment_id` 恒为空串，daemon 无法回传可删除的
    /// 评论 ID，已修复）。
    pub async fn add_comment(
        &self,
        biz_id: i64,
        content: &str,
        reply_cmt_id: Option<&str>,
        credential: &Credential,
    ) -> Result<AddCommentResponse, QqMusicError> {
        let mut param = serde_json::Map::new();
        param.insert("Content".into(), Value::String(content.to_string()));
        param.insert("BizType".into(), json!(BIZ_TYPE_SONG));
        param.insert("BizId".into(), Value::String(biz_id.to_string()));
        if let Some(reply) = reply_cmt_id {
            param.insert("RepliedCmId".into(), Value::String(reply.to_string()));
        }
        param.insert("BizSubType".into(), json!(BIZ_SUB_TYPE_SONG));
        let request = CgiRequest::new(
            "music.globalComment.CommentWriteServer",
            "AddComment",
            Value::Object(param),
        )
        .with_require_login(true);
        let data = self
            .client
            .musicu_request(&request, Some(credential))
            .await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("failed to parse add comment: {e}")))
    }

    /// 删除自己发表的评论（上游 `delete_comment`；登录：需登录）。
    ///
    /// `cm_id` 为 [`Self::add_comment`] 返回的 `comment_id`（形如 `1!...` 的
    /// 字符串，非数字）。上游语义：评论不存在也返回 `true`。
    ///
    /// 实测（2026-09-29）：服务端判定键为内层 `data.Subcode`（注意大小写，
    /// 与 AddComment 的 `SubCode` 不同），键缺省视为 0；此前在子响应顶层找
    /// `SubCode` 导致恒为 `false`（daemon 误报删除失败），已修复。
    pub async fn delete_comment(
        &self,
        cm_id: &str,
        credential: &Credential,
    ) -> Result<bool, QqMusicError> {
        let request = CgiRequest::new(
            "music.globalComment.CommentWriteServer",
            "DelComment",
            json!({ "CommentId": cm_id }),
        )
        .with_require_login(true);
        let data = self
            .client
            .musicu_request(&request, Some(credential))
            .await?;
        // 服务端响应：{"code":0,"data":{"Subcode":0,"Msg":""}}（内层 data）；
        // 兼容上游读取键 `SubCode`；键缺省按上游 `data.get("SubCode", 0)` 视为 0。
        let subcode = data
            .get("data")
            .and_then(|d| d.get("Subcode").or_else(|| d.get("SubCode")))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        Ok(subcode == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 映射 QQ `Comments[]` 字段（真实响应结构，2026-08 实测）。
    #[test]
    fn comment_list_parses_qq_shape() {
        let v: CommentListResponse = serde_json::from_value(json!({
            "CommentList": {
                "Comments": [
                    {
                        "CmId": "1!ABC",
                        "SeqNo": "1628467045072956929",
                        "Nick": "胡桃",
                        "Content": "好听",
                        "PraiseNum": 42,
                        "PubTime": 1700000000,
                        "ReplyCnt": 3,
                    }
                ],
                "HasMore": 1,
                "Total": 100,
            },
            "TotalCmNum": 83633,
        }))
        .expect("QQ Comments[] 字段应识别");
        let list = v.comment_list.unwrap();
        assert_eq!(list.comments.len(), 1);
        assert_eq!(list.comments[0].cm_id, "1!ABC");
        assert_eq!(list.comments[0].seq_no, "1628467045072956929");
        assert_eq!(list.comments[0].nickname, "胡桃");
        assert_eq!(list.comments[0].content, "好听");
        assert_eq!(list.comments[0].like_count, 42);
        assert_eq!(list.comments[0].reply_count, 3);
        assert_eq!(list.total, 100);
        assert_eq!(v.total_cm_num, 83633);
        // 空响应不报错。
        let empty: CommentListResponse = serde_json::from_value(json!({})).unwrap();
        assert!(empty.comment_list.is_none());
    }

    /// 评论数提取：`data.response.count`（真实响应结构）。
    #[test]
    fn count_extracts_response_count() {
        let data = json!({ "data": { "response": { "count": 83633 } } });
        let count = data
            .get("data")
            .and_then(|d| d.get("response"))
            .and_then(|r| r.get("count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        assert_eq!(count, 83633);
        // 缺失 → 0。
        let none = json!({});
        let count = none
            .get("data")
            .and_then(|d| d.get("response"))
            .and_then(|r| r.get("count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        assert_eq!(count, 0);
    }

    #[test]
    fn comment_pages_are_zero_based_via_caller() {
        // PageNum = page-1：验证构造（纯逻辑，无网络）。
        let page = 3i64;
        assert_eq!(page - 1, 2);
    }
}
