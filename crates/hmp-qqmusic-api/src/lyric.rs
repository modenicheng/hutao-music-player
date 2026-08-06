//! 歌词模块（对应上游 `modules/lyric.py`）。
//!
//! 歌词响应中的 `lyric`/`trans`/`roma` 字段可能为加密 QRC（`crypt=1`），
//! 解析时自动调用 `qrc_decrypt` 解密（对应上游 model validator）。

use serde::Deserialize;
use serde_json::json;

use crate::algorithms::qrc_decrypt;
use crate::client::QqMusicClient;
use crate::error::QqMusicError;
use crate::protocol::cgi::CgiRequest;

/// 歌词响应（上游 `GetLyricResponse`）。
#[derive(Clone, Debug, Deserialize)]
pub struct GetLyricResponse {
    /// 歌曲 ID。
    #[serde(default, alias = "songID")]
    pub songid: i64,
    /// 原始歌词内容（LRC 文本，已解密）。
    #[serde(default)]
    pub lyric: String,
    /// 翻译歌词内容。
    #[serde(default)]
    pub trans: String,
    /// 罗马音歌词内容。
    #[serde(default)]
    pub roma: String,
    /// 助唱标注歌词。
    #[serde(default, alias = "singingAnnotationsLyric")]
    pub singing_annotations_lyric: String,
    /// LRC 歌词更新时间戳。
    #[serde(default)]
    pub lrc_t: i64,
    /// QRC 歌词更新时间戳。
    #[serde(default)]
    pub qrc_t: i64,
    /// 翻译歌词更新时间戳。
    #[serde(default)]
    pub trans_t: i64,
    /// 罗马音歌词更新时间戳。
    #[serde(default)]
    pub roma_t: i64,
    /// 是否有歌词贡献者。
    #[serde(default, alias = "hasContributor")]
    pub has_contributor: bool,
    /// 是否有翻译贡献者。
    #[serde(default, alias = "hasTransContributor")]
    pub has_trans_contributor: bool,
    /// 是否有多风格翻译歌词。
    #[serde(default, alias = "hasMultiTrans")]
    pub has_multi_trans: bool,
}

impl GetLyricResponse {
    /// 解析并解密歌词字段（上游 model validator `_decrypt_lyrics`）。
    ///
    /// `lyric`/`trans`/`roma`/`singing_annotations_lyric` 为 QRC hex 时解密；
    /// 非 hex 或解密失败时保留原文。
    pub fn decrypt_fields(mut self) -> Self {
        self.lyric = decrypt_field(&self.lyric);
        self.trans = decrypt_field(&self.trans);
        self.roma = decrypt_field(&self.roma);
        self.singing_annotations_lyric = decrypt_field(&self.singing_annotations_lyric);
        self
    }
}

/// 尝试解密单个歌词字段（失败保留原文）。
fn decrypt_field(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    // 明文 LRC 通常以 `[` 开头，直接返回
    if value.starts_with('[') {
        return value.to_owned();
    }
    qrc_decrypt(value).unwrap_or_else(|_| value.to_owned())
}

/// 歌词 API（对应上游 `LyricApi`）。
pub struct LyricApi<'a> {
    client: &'a QqMusicClient,
}

impl<'a> LyricApi<'a> {
    /// 构造歌词 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 获取歌词原始数据（上游 `get_lyric`）。
    ///
    /// `value` 为歌曲 ID（纯数字）或 MID。
    pub async fn get_lyric(
        &self,
        value: &str,
        song_type: i64,
        qrc: bool,
        trans: bool,
        roma: bool,
        singing_annotations: bool,
    ) -> Result<GetLyricResponse, QqMusicError> {
        let mut param = json!({
            "crypt": 1,
            "lrc_t": 0,
            "qrc": qrc as i64,
            "qrc_t": 0,
            "roma": roma as i64,
            "roma_t": 0,
            "trans": trans as i64,
            "trans_t": 0,
            "needSingingAnnotations": singing_annotations,
            "type": song_type,
        });
        if value.chars().all(|c| c.is_ascii_digit()) {
            param["songId"] = json!(value.parse::<i64>().unwrap_or(0));
        } else {
            param["songMid"] = json!(value);
        }

        let request = CgiRequest::new(
            "music.musichallSong.PlayLyricInfo",
            "GetPlayLyricInfo",
            param,
        );
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let resp: GetLyricResponse = serde_json::from_value(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("lyric 解析失败: {e}")))?;
        Ok(resp.decrypt_fields())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_decrypts_real_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lyric/encrypted.json"
        );
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: GetLyricResponse = serde_json::from_value(data.clone()).unwrap();
        let resp = resp.decrypt_fields();

        assert_eq!(resp.songid, 186016);
        assert!(
            resp.lyric.contains('['),
            "decrypted lyric should be LRC, got prefix: {}",
            &resp.lyric[..resp.lyric.len().min(60)]
        );
        // 解密后的 LRC 应包含歌曲标题与时间戳行
        assert!(
            resp.lyric.contains("[ti:") || resp.lyric.contains("[ar:") || resp.lyric.contains(']')
        );
    }

    #[test]
    fn decrypt_field_keeps_plaintext_lrc() {
        let plain = "[ti:test]\n[00:01.00]hello\n";
        assert_eq!(decrypt_field(plain), plain);
    }

    #[test]
    fn decrypt_field_handles_empty() {
        assert_eq!(decrypt_field(""), "");
    }
}
