//! 搜索（对应上游 `qqmusic_api/modules/search.py` 的 `quick_search`）。
//!
//! `quick_search` 走 `c.y.qq.com/splcloud/fcgi-bin/smartbox_new.fcg`（GET），
//! 免登录，返回歌曲/专辑/歌手/MV 快速匹配。

use serde_json::Value;

use crate::error::QqMusicError;

/// 快速搜索单曲结果。
#[derive(Clone, Debug, PartialEq)]
pub struct QuickSong {
    /// songmid（如 `0039MnYb0qxYhV`）。
    pub mid: String,
    /// 歌曲名。
    pub name: String,
    /// 歌手名（单一字符串）。
    pub singer: String,
}

/// 快速搜索专辑结果。
#[derive(Clone, Debug, PartialEq)]
pub struct QuickAlbum {
    /// albummid。
    pub mid: String,
    /// 专辑名。
    pub name: String,
    /// 封面 URL。
    pub pic: Option<String>,
    /// 歌手名。
    pub singer: String,
}

/// 快速搜索歌手结果。
#[derive(Clone, Debug, PartialEq)]
pub struct QuickSinger {
    /// singermid。
    pub mid: String,
    /// 歌手名。
    pub name: String,
    /// 头像 URL。
    pub pic: Option<String>,
}

/// 快速搜索响应。
#[derive(Clone, Debug, Default)]
pub struct QuickSearch {
    /// 匹配到的单曲。
    pub songs: Vec<QuickSong>,
    /// 匹配到的专辑。
    pub albums: Vec<QuickAlbum>,
    /// 匹配到的歌手。
    pub singers: Vec<QuickSinger>,
}

impl QuickSearch {
    /// 从 smartbox 响应 JSON 解析（`data.song.itemlist` 等）。
    pub fn from_value(body: &Value) -> Result<Self, QqMusicError> {
        let data = body
            .get("data")
            .and_then(|v| v.as_object())
            .ok_or_else(|| {
                QqMusicError::InvalidResponse("smartbox response missing data".into())
            })?;

        let songs = parse_song_list(data.get("song"));
        let albums = parse_album_list(data.get("album"));
        let singers = parse_singer_list(data.get("singer"));

        Ok(QuickSearch {
            songs,
            albums,
            singers,
        })
    }
}

fn parse_song_list(section: Option<&Value>) -> Vec<QuickSong> {
    let Some(list) = section
        .and_then(|v| v.get("itemlist"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| {
            Some(QuickSong {
                mid: item.get("mid")?.as_str()?.to_owned(),
                name: item.get("name")?.as_str()?.to_owned(),
                singer: item
                    .get("singer")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned(),
            })
        })
        .collect()
}

fn parse_album_list(section: Option<&Value>) -> Vec<QuickAlbum> {
    let Some(list) = section
        .and_then(|v| v.get("itemlist"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| {
            Some(QuickAlbum {
                mid: item.get("mid")?.as_str()?.to_owned(),
                name: item.get("name")?.as_str()?.to_owned(),
                pic: item.get("pic").and_then(|v| v.as_str()).map(str::to_owned),
                singer: item
                    .get("singer")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned(),
            })
        })
        .collect()
}

fn parse_singer_list(section: Option<&Value>) -> Vec<QuickSinger> {
    let Some(list) = section
        .and_then(|v| v.get("itemlist"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| {
            Some(QuickSinger {
                mid: item.get("mid")?.as_str()?.to_owned(),
                name: item.get("name")?.as_str()?.to_owned(),
                pic: item.get("pic").and_then(|v| v.as_str()).map(str::to_owned),
            })
        })
        .collect()
}
