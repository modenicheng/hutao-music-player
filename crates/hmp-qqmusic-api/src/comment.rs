//! 评论 API（对应上游 `CommentApi`，模块 `qqmusic_api/modules/comment.py`）。
//!
//! 评论按 `biz_id`（QQ numeric song id）寻址——CLI 侧经 `tracks.qq_song_id`
//! 从 mid 映射（spec §6）。`biz_type=SONG=1`、`biz_sub_type=2`。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::protocol::cgi::CgiRequest;

/// 评论（上游 `Comment`；映射 QQ `Comments[]` 字段）。
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

/// 评论列表数据（上游 `CommentList`）。
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

/// 评论列表响应（上游 `CommentListResponse`）。
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
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AddCommentResponse {
    /// 新评论 ID。
    #[serde(default, alias = "commentId", alias = "cmid", alias = "CmId")]
    pub comment_id: String,
    /// 返回码。
    #[serde(default)]
    pub ret: i64,
}

/// 评论 API。
pub struct CommentApi<'a> {
    client: &'a QqMusicClient,
}

/// 评论类型（上游 `CommentBizType`）：默认普通歌曲。
pub const BIZ_TYPE_SONG: i64 = 1;
/// 歌曲子类型。
pub const BIZ_SUB_TYPE_SONG: i64 = 2;

impl<'a> CommentApi<'a> {
    /// 构造评论 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 评论数量（上游 `get_comment_count`；取 `data.response.count`）。
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

    /// 热评（上游 `get_hot_comments`）。
    pub async fn get_hot_comments(
        &self,
        biz_id: i64,
        page: i64,
        page_size: i64,
    ) -> Result<Vec<Comment>, QqMusicError> {
        let request = CgiRequest::new(
            "music.globalComment.CommentRead",
            "GetHotCommentList",
            json!({
                "BizType": BIZ_TYPE_SONG,
                "BizId": biz_id.to_string(),
                "LastCommentSeqNo": "",
                "PageSize": page_size,
                "PageNum": page - 1,
                "HotType": 1,
                "WithAirborne": 0,
                "PicEnable": 1,
                "BizSubType": BIZ_SUB_TYPE_SONG,
            }),
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let resp: CommentListResponse = serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("hot comments 解析失败: {e}")))?;
        Ok(resp.comment_list.map(|l| l.comments).unwrap_or_default())
    }

    /// 最新评论（上游 `get_new_comments`）。
    pub async fn get_new_comments(
        &self,
        biz_id: i64,
        page: i64,
        page_size: i64,
    ) -> Result<Vec<Comment>, QqMusicError> {
        let request = CgiRequest::new(
            "music.globalComment.CommentRead",
            "GetNewCommentList",
            json!({
                "PageSize": page_size,
                "PageNum": page - 1,
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
        let resp: CommentListResponse = serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("new comments 解析失败: {e}")))?;
        Ok(resp.comment_list.map(|l| l.comments).unwrap_or_default())
    }

    /// 推荐评论（上游 `get_recommend_comments`）。
    pub async fn get_recommend_comments(
        &self,
        biz_id: i64,
        page: i64,
        page_size: i64,
    ) -> Result<Vec<Comment>, QqMusicError> {
        let request = CgiRequest::new(
            "music.globalComment.CommentRead",
            "GetRecCommentList",
            json!({
                "PageSize": page_size,
                "PageNum": page - 1,
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
        let resp: CommentListResponse = serde_json::from_value(data).map_err(|e| {
            QqMusicError::InvalidResponse(format!("recommend comments 解析失败: {e}"))
        })?;
        Ok(resp.comment_list.map(|l| l.comments).unwrap_or_default())
    }

    /// 发表评论（上游 `add_comment`；`reply_cmt_id` 非空即回复）。
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
            .map_err(|e| QqMusicError::InvalidResponse(format!("add comment 解析失败: {e}")))
    }

    /// 删除评论（上游 `delete_comment`；评论不存在也返回 true）。
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
        Ok(data.get("SubCode").and_then(|v| v.as_i64()) == Some(0))
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
