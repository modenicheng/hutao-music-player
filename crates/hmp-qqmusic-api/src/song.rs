//! 歌曲模块（对应上游 `modules/song.py`）。
//!
//! 提供歌曲详情、批量查询、播放 URL 获取等接口。
//! 凭证按 §6.4 解耦：需要登录态的接口由调用方显式传入 `&Credential`。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::models::Song;
use crate::protocol::cgi::CgiRequest;

/// 歌曲文件类型（上游 `BaseSongFileType` / `SongFileType` /
/// `EncryptedSongFileType` / `SpecialSongFileType` 合并）。
///
/// 服务端现实：高音质（无损 FLAC、臻品母带、全景声、高码率 OGG、DTS:X、
/// AICodec）只提供**加密文件**（`.mflac`/`.mgg`/`.mnac`/`.mmp4`），
/// `is_encrypted = true` 时取流自动走 `music.vkey.GetEVkey`（`CgiGetEVkey`）
/// 并返回 `ekey` 供播放器解密。上游普通组的明文变体（`F000`/`AI00` 等）
/// 服务端已停发，故不提供；低音质（MP3/AAC/试听）保持明文。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SongFileType {
    /// 文件编码前缀（如 `M500`）。
    pub s: &'static str,
    /// 文件后缀（如 `.mp3`）。
    pub e: &'static str,
    /// 是否为加密文件（取流走 `CgiGetEVkey`）。
    pub is_encrypted: bool,
}

impl SongFileType {
    const fn new(s: &'static str, e: &'static str, is_encrypted: bool) -> Self {
        Self { s, e, is_encrypted }
    }
}

/// 普通与加密歌曲文件类型（上游 `SongFileType` + `EncryptedSongFileType`）。
impl SongFileType {
    /// DTS:X 音效（加密，上游 `EncryptedSongFileType.DTS_X`）。
    pub const DTS_X: Self = Self::new("DTM3", ".mmp4", true);
    /// 黑胶（加密，上游 `EncryptedSongFileType.VINYL`）。
    pub const VINYL: Self = Self::new("V0M0", ".mflac", true);
    /// 臻品母带（加密）。
    pub const MASTER: Self = Self::new("AIM0", ".mflac", true);
    /// 臻品音质 2.0（加密）。
    pub const ATMOS_2: Self = Self::new("Q0M0", ".mflac", true);
    /// 臻品全景声 5.1（加密）。
    pub const ATMOS_51: Self = Self::new("Q0M1", ".mflac", true);
    /// 臻品全景声 7.1（加密）。
    pub const ATMOS_71: Self = Self::new("Q0M3", ".mgg", true);
    /// 杜比全景声（加密）。
    pub const ATMOS_DB: Self = Self::new("D0M4", ".mmp4", true);
    /// 腾讯自研 AICodec（加密）。
    pub const NAC: Self = Self::new("TLM1", ".mnac", true);
    /// SQ 无损音质（加密）。
    pub const FLAC: Self = Self::new("F0M0", ".mflac", true);
    /// SQ 无损（OGG 640k，加密）。
    pub const OGG_640: Self = Self::new("O8M1", ".mgg", true);
    /// HQ 高品质 OGG 320k（加密）。
    pub const OGG_320: Self = Self::new("O8M0", ".mgg", true);
    /// HQ 高品质 OGG 192k（加密）。
    pub const OGG_192: Self = Self::new("O6M0", ".mgg", true);
    /// 流畅音质 OGG 96k（加密）。
    pub const OGG_96: Self = Self::new("O4M0", ".mgg", true);
    /// HQ 高品质 MP3 320k（明文）。
    pub const MP3_320: Self = Self::new("M800", ".mp3", false);
    /// 标准音质 MP3 128k（明文）。
    pub const MP3_128: Self = Self::new("M500", ".mp3", false);
    /// HQ 高品质 AAC 192k（明文）。
    pub const AAC_192: Self = Self::new("C600", ".m4a", false);
    /// 流畅音质 AAC 96k（明文）。
    pub const AAC_96: Self = Self::new("C400", ".m4a", false);
    /// 低品质 AAC 48k（明文）。
    pub const AAC_48: Self = Self::new("C200", ".m4a", false);
}

/// 特殊歌曲文件类型（上游 `SpecialSongFileType`，均明文）。
impl SongFileType {
    /// 歌曲试听。
    pub const TRY: Self = Self::new("RS02", ".mp3", false);
    /// SQ 无损试听。
    pub const TRY_OGG_640: Self = Self::new("O802", ".ogg", false);
    /// 纯人声/伴奏轨道。
    pub const ACCOM: Self = Self::new("O801", ".ogg", false);
}

/// 歌曲文件信息（上游 `SongFileInfo`）。
#[derive(Clone, Debug)]
pub struct SongFileInfo {
    /// 歌曲 MID。
    pub mid: String,
    /// 文件类型（缺省用请求级 file_type）。
    pub file_type: Option<SongFileType>,
    /// 歌曲类型。
    pub song_type: i64,
    /// 媒体文件 mid（缺省用 `mid` 拼接）。
    pub media_mid: Option<String>,
}

/// 歌曲查询信息（上游 `SongQueryInfo`）。
#[derive(Clone, Debug)]
pub struct SongQueryInfo {
    /// 歌曲 ID。
    pub id: Option<i64>,
    /// 歌曲 MID。
    pub mid: Option<String>,
    /// 歌曲类型。
    pub song_type: i64,
}

/// 单个文件授权结果（上游 `UrlinfoItem`）。
#[derive(Clone, Debug, Deserialize)]
pub struct UrlinfoItem {
    /// 歌曲 mid。
    #[serde(default)]
    pub mid: String,
    /// 请求中的歌曲 mid（上游 `songmid`，与 `mid` 可能不同）。
    #[serde(default)]
    pub songmid: String,
    /// 请求的目标文件名。
    #[serde(default)]
    pub filename: String,
    /// 相对下载路径（需与 CDN 域名拼接）。
    #[serde(default)]
    pub purl: String,
    /// 资源访问令牌。
    #[serde(default)]
    pub vkey: String,
    /// 加密资源解密密钥。
    #[serde(default)]
    pub ekey: String,
    /// 单个文件业务结果码（0=成功，104003=无权限等）。
    #[serde(default)]
    pub result: i64,
}

/// 歌曲播放地址响应（上游 `GetSongUrlsResponse`）。
#[derive(Clone, Debug, Deserialize)]
pub struct GetSongUrlsResponse {
    /// 链接过期时间（秒）。
    #[serde(default)]
    pub expiration: i64,
    /// 每个目标文件的授权与路径信息。
    #[serde(default, alias = "midurlinfo")]
    pub data: Vec<UrlinfoItem>,
}

impl GetSongUrlsResponse {
    /// 拼接完整播放 URL（上游使用 `sip[0]` + purl，缺省回落官方域名）。
    ///
    /// 返回与 `data` 等长的 URL 列表；无 `purl` 的条目为 `None`。
    pub fn build_urls(&self) -> Vec<Option<String>> {
        let domain = "https://isure.stream.qqmusic.qq.com/";
        self.data
            .iter()
            .map(|item| {
                if item.purl.is_empty() {
                    None
                } else {
                    Some(format!("{domain}{}", item.purl))
                }
            })
            .collect()
    }
}

/// 歌曲详情内容项（上游 `ContentItem`）。
#[derive(Clone, Debug, Deserialize)]
pub struct ContentItem {
    /// 内容项 ID。
    #[serde(default)]
    pub id: i64,
    /// 内容项值。
    #[serde(default)]
    pub value: String,
    /// 展示类型。
    #[serde(default)]
    pub show_type: i64,
    /// 跳转链接。
    #[serde(default)]
    pub jumpurl: String,
}

/// 歌曲详情响应（上游 `GetSongDetailResponse`）。
#[derive(Clone, Debug, Deserialize)]
pub struct GetSongDetailResponse {
    /// 发行公司信息。
    #[serde(default)]
    pub company: Vec<ContentItem>,
    /// 音乐流派信息。
    #[serde(default)]
    pub genre: Vec<ContentItem>,
    /// 歌曲简介。
    #[serde(default)]
    pub intro: Vec<ContentItem>,
    /// 语言信息。
    #[serde(default)]
    pub lan: Vec<ContentItem>,
    /// 发布时间。
    #[serde(default)]
    pub pub_time: Vec<ContentItem>,
    /// 额外信息。
    #[serde(default)]
    pub extras: serde_json::Map<String, Value>,
    /// 歌曲基本信息。
    #[serde(default, alias = "track_info")]
    pub track: Song,
}

/// 歌曲 API（对应上游 `SongApi`）。
pub struct SongApi<'a> {
    client: &'a QqMusicClient,
}

/// 歌曲播放 URL 的 CDN 回退域名（上游 `_SONG_URL_FALLBACK_DOMAIN`）。
pub const SONG_URL_FALLBACK_DOMAIN: &str = "https://isure.stream.qqmusic.qq.com/";

impl<'a> SongApi<'a> {
    /// 构造歌曲 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 批量获取歌曲信息（上游 `query_song`）。
    pub async fn query_song(&self, song_info: &[SongQueryInfo]) -> Result<Vec<Song>, QqMusicError> {
        if song_info.is_empty() {
            return Err(QqMusicError::InvalidResponse("song_info 不能为空".into()));
        }

        let mut ids = Vec::new();
        let mut mids = Vec::new();
        let mut types = Vec::new();
        for item in song_info {
            match (item.id, item.mid.as_deref()) {
                (Some(id), None) => ids.push(id),
                (None, Some(mid)) => mids.push(mid),
                _ => {
                    return Err(QqMusicError::InvalidResponse(
                        "SongQueryInfo 必须提供 id 或 mid 且不能同时提供".into(),
                    ));
                }
            }
            types.push(item.song_type);
        }

        let mut param = json!({
            "ctx": 0,
            "client": 1,
            "types": types,
            "modify_stamp": vec![0; song_info.len()],
        });
        if !ids.is_empty() {
            param["ids"] = Value::Array(ids.iter().map(|v| json!(v)).collect());
        }
        if !mids.is_empty() {
            param["mids"] = Value::Array(mids.iter().map(|v| json!(v)).collect());
        }

        let request = CgiRequest::new("music.trackInfo.UniformRuleCtrl", "CgiGetTrackInfo", param);
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        let tracks = data
            .get("tracks")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| serde_json::from_value::<Song>(t.clone()).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(tracks)
    }

    /// 获取歌曲详细信息（上游 `get_detail`，固定 Web 平台）。
    pub async fn get_detail(&self, value: &str) -> Result<GetSongDetailResponse, QqMusicError> {
        let param = if value.chars().all(|c| c.is_ascii_digit()) {
            json!({"song_id": value.parse::<i64>().unwrap_or(0)})
        } else {
            json!({"song_mid": value})
        };

        let request = CgiRequest::new("music.pf_song_detail_svr", "get_song_detail_yqq", param);
        let data = self.client.musicu_request(&request, None).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<GetSongDetailResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("song detail 解析失败: {e}")))
    }

    /// 获取歌曲文件链接（上游 `get_song_urls`）。
    ///
    /// 免登录时通常仅试听类型（如 [`SongFileType::TRY`]）可用；
    /// 完整音质需要登录态，由调用方传入 `credential`。
    pub async fn get_song_urls(
        &self,
        file_info: &[SongFileInfo],
        file_type: SongFileType,
        credential: Option<&Credential>,
    ) -> Result<GetSongUrlsResponse, QqMusicError> {
        // 加密类型走 CgiGetEVkey（上游按请求级 file_type 判断，item 覆盖仅影响文件名）
        let (module, method) = if file_type.is_encrypted {
            ("music.vkey.GetEVkey", "CgiGetEVkey")
        } else {
            ("music.vkey.GetVkey", "UrlGetVkey")
        };
        let mut songmid = Vec::new();
        let mut filename = Vec::new();
        let mut songtype = Vec::new();
        for item in file_info {
            songmid.push(item.mid.clone());
            let final_type = item.file_type.unwrap_or(file_type);
            let fname = match &item.media_mid {
                Some(media_mid) if !media_mid.is_empty() => {
                    format!("{}{}{}", final_type.s, media_mid, final_type.e)
                }
                _ => format!("{}{}{}{}", final_type.s, item.mid, item.mid, final_type.e),
            };
            filename.push(fname);
            songtype.push(item.song_type);
        }

        let request = CgiRequest::new(
            module,
            method,
            json!({
                "uin": credential.map(|c| c.str_musicid.clone()).unwrap_or_default(),
                "filename": filename,
                "guid": uuid4_str(),
                "songmid": songmid,
                "songtype": songtype,
                "ctx": 0,
            }),
        );
        let data = self.client.musicu_request(&request, credential).await?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        serde_json::from_value::<GetSongUrlsResponse>(data)
            .map_err(|e| QqMusicError::InvalidResponse(format!("song urls 解析失败: {e}")))
    }
}

/// 生成 32 位随机 GUID（上游 `get_guid`）。
pub fn get_guid() -> String {
    uuid4_str()
}

fn uuid4_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!(
        "{:08x}{:08x}{:08x}{:08x}",
        seed as u32,
        (seed >> 32) as u32,
        seed.wrapping_mul(0x9e3779b97f4a7c15) as u32,
        (seed >> 16) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::assertions_on_constants)] // 常量契约测试
    #[test]
    fn plain_file_type_constants_match_upstream() {
        assert_eq!(SongFileType::MP3_128.s, "M500");
        assert_eq!(SongFileType::MP3_128.e, ".mp3");
        assert!(!SongFileType::MP3_128.is_encrypted);
        assert_eq!(SongFileType::MP3_320.s, "M800");
        assert_eq!(SongFileType::AAC_192.s, "C600");
        assert_eq!(SongFileType::TRY.s, "RS02");
        assert!(!SongFileType::TRY.is_encrypted);
    }

    #[allow(clippy::assertions_on_constants)] // 常量契约测试
    #[test]
    fn encrypted_file_type_constants_match_upstream() {
        // 对齐上游 EncryptedSongFileType
        assert_eq!(SongFileType::FLAC.s, "F0M0");
        assert_eq!(SongFileType::FLAC.e, ".mflac");
        assert!(SongFileType::FLAC.is_encrypted);
        assert_eq!(SongFileType::MASTER.s, "AIM0");
        assert_eq!(SongFileType::MASTER.e, ".mflac");
        assert_eq!(SongFileType::VINYL.s, "V0M0");
        assert_eq!(SongFileType::OGG_640.s, "O8M1");
        assert_eq!(SongFileType::OGG_640.e, ".mgg");
        assert_eq!(SongFileType::OGG_320.s, "O8M0");
        assert_eq!(SongFileType::OGG_192.s, "O6M0");
        assert_eq!(SongFileType::OGG_96.s, "O4M0");
        assert_eq!(SongFileType::DTS_X.s, "DTM3");
        assert_eq!(SongFileType::DTS_X.e, ".mmp4");
        assert_eq!(SongFileType::ATMOS_2.s, "Q0M0");
        assert_eq!(SongFileType::ATMOS_51.s, "Q0M1");
        assert_eq!(SongFileType::ATMOS_71.s, "Q0M3");
        assert_eq!(SongFileType::ATMOS_DB.s, "D0M4");
        assert_eq!(SongFileType::NAC.s, "TLM1");
        assert_eq!(SongFileType::NAC.e, ".mnac");
        for t in [
            SongFileType::FLAC,
            SongFileType::MASTER,
            SongFileType::VINYL,
            SongFileType::OGG_640,
            SongFileType::OGG_320,
            SongFileType::OGG_192,
            SongFileType::OGG_96,
            SongFileType::DTS_X,
            SongFileType::ATMOS_2,
            SongFileType::ATMOS_51,
            SongFileType::ATMOS_71,
            SongFileType::ATMOS_DB,
            SongFileType::NAC,
        ] {
            assert!(t.is_encrypted, "{} should be encrypted", t.s);
        }
    }

    #[test]
    fn song_urls_parses_real_evkey_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/song/evkey_encrypted.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: GetSongUrlsResponse = serde_json::from_value(data.clone()).unwrap();

        assert!(resp.expiration > 0);
        assert_eq!(resp.data.len(), 1);
        let item = &resp.data[0];
        // 免登录加密取流：101404 = 需要登录
        assert_eq!(item.result, 101404);
        assert_eq!(item.filename, "F0M0003BEgWZ2eI1Qo003BEgWZ2eI1Qo.mflac");
        assert!(item.filename.starts_with("F0M0"));
    }

    #[test]
    fn build_urls_joins_purl_with_domain() {
        let resp = GetSongUrlsResponse {
            expiration: 7200,
            data: vec![
                UrlinfoItem {
                    mid: "mid1".into(),
                    songmid: "mid1".into(),
                    filename: "f1".into(),
                    purl: "RS02mid1.mp3?guid=abc".into(),
                    vkey: "vkey1".into(),
                    ekey: String::new(),
                    result: 0,
                },
                UrlinfoItem {
                    mid: "mid2".into(),
                    songmid: "mid2".into(),
                    filename: "f2".into(),
                    purl: String::new(),
                    vkey: String::new(),
                    ekey: String::new(),
                    result: 104003,
                },
            ],
        };
        let urls = resp.build_urls();
        assert_eq!(
            urls[0].as_deref(),
            Some("https://isure.stream.qqmusic.qq.com/RS02mid1.mp3?guid=abc")
        );
        assert!(urls[1].is_none());
    }

    #[test]
    fn song_detail_parses_real_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/song/detail_by_id.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let detail: GetSongDetailResponse = serde_json::from_value(data.clone()).unwrap();
        assert_eq!(detail.track.id, 186016);
        assert_eq!(detail.track.name, "开始懂了");
        assert_eq!(detail.track.singer.len(), 1);
        assert_eq!(detail.track.singer[0].name, "孙燕姿");
        assert_eq!(detail.track.album.name, "孙燕姿经典全纪录 主打精华版");
        assert!(detail.track.interval > 0);
        assert!(detail.track.file.media_mid.len() > 4);
    }

    #[test]
    fn song_urls_parses_real_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/song/urls_try.json"
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let data = &body["req_0"]["data"];
        let resp: GetSongUrlsResponse = serde_json::from_value(data.clone()).unwrap();
        assert_eq!(resp.expiration, 7200);
        assert_eq!(resp.data.len(), 1);
        let item = &resp.data[0];
        assert_eq!(item.result, 0);
        assert!(item.purl.contains("vkey="), "purl should embed vkey");
        let urls = resp.build_urls();
        assert!(urls[0].as_deref().unwrap().starts_with("https://"));
    }
}
