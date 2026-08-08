//! MPRIS 元数据（`Track` → `a{sv}`）。

use hmp_core::{AudioQuality, Track};
use zvariant::{OwnedValue, Value};

/// MPRIS 播放状态字符串（spec: Playing/Paused/Stopped）。
pub fn playback_status(status: hmp_core::PlaybackStatus) -> &'static str {
    match status {
        hmp_core::PlaybackStatus::Playing => "Playing",
        hmp_core::PlaybackStatus::Paused => "Paused",
        _ => "Stopped",
    }
}

/// MPRIS 循环状态字符串（spec: None/Track/Playlist）。
pub fn loop_status(mode: hmp_core::LoopMode) -> &'static str {
    match mode {
        hmp_core::LoopMode::None => "None",
        hmp_core::LoopMode::List => "Playlist",
        hmp_core::LoopMode::Track => "Track",
    }
}

/// 从领域曲目构建 MPRIS Metadata 字典（`mpris:trackid` 必填）。
pub fn metadata_from_track(track: &Track) -> Vec<(&'static str, OwnedValue)> {
    let mut meta: Vec<(&'static str, OwnedValue)> = Vec::new();

    // mpris:trackid —— 必填，格式 /org/hmp/track/<id>
    let track_id_path: zvariant::ObjectPath = format!("/org/hmp/track/{}", track.id)
        .try_into()
        .unwrap_or_else(|_| "/org/hmp/track/unknown".try_into().expect("static path"));
    meta.push((
        "mpris:trackid",
        OwnedValue::try_from(Value::ObjectPath(track_id_path)).expect("object path"),
    ));

    if !track.title.is_empty() {
        meta.push((
            "xesam:title",
            OwnedValue::try_from(Value::from(track.title.clone())).expect("str"),
        ));
    }
    if !track.artists.is_empty() {
        // 必须为 as（字符串数组）：Vec<String> 签名 s；
        // Vec<Value> 会产生 av（变体数组），部分客户端（Quickshell/DMS）无法解析。
        let artists: Vec<String> = track.artists.iter().map(|a| a.name.clone()).collect();
        meta.push((
            "xesam:artist",
            OwnedValue::try_from(Value::from(artists.clone())).expect("arr"),
        ));
        // xesam:albumArtist —— 数据源无独立专辑歌手时回退为歌曲歌手
        // （QQ 音乐详情不提供专辑歌手字段，多数 MPRIS 客户端依赖该键展示）。
        meta.push((
            "xesam:albumArtist",
            OwnedValue::try_from(Value::from(artists)).expect("arr"),
        ));
    }
    if let Some(album) = &track.album {
        meta.push((
            "xesam:album",
            OwnedValue::try_from(Value::from(album.name.clone())).expect("str"),
        ));
    }
    if let Some(cover) = &track.cover {
        meta.push((
            "mpris:artUrl",
            OwnedValue::try_from(Value::from(cover.url.clone())).expect("str"),
        ));
    }
    if let Some(url) = &track.url {
        meta.push((
            "xesam:url",
            OwnedValue::try_from(Value::from(url.clone())).expect("str"),
        ));
    }
    if let Some(duration) = track.duration {
        // mpris:length 单位微秒
        let us = duration.as_micros().min(u64::MAX as u128) as u64;
        meta.push((
            "mpris:length",
            OwnedValue::try_from(Value::from(us)).expect("u64"),
        ));
    }
    if let Some(q) = track.available_qualities.first() {
        meta.push((
            "xesam:audioQuality",
            OwnedValue::try_from(Value::from(quality_label(q))).expect("str"),
        ));
    }
    meta
}

/// 音质展示标签。
pub fn quality_label(q: &AudioQuality) -> String {
    match q {
        AudioQuality::Master => "臻品母带".into(),
        AudioQuality::HiRes => "Hi-Res".into(),
        AudioQuality::Atmos => "全景声".into(),
        AudioQuality::Flac => "无损".into(),
        AudioQuality::Aac => "AAC".into(),
        AudioQuality::Mp3_320 => "HQ 320k".into(),
        AudioQuality::Mp3_128 => "标准 128k".into(),
        AudioQuality::Unknown(s) => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmp_core::{AlbumRef, ArtistId, ArtistRef, CoverRef, TrackId};

    fn track() -> Track {
        Track {
            id: TrackId::new("mid-1"),
            title: "开始懂了".into(),
            artists: vec![ArtistRef {
                id: ArtistId::new("s-1"),
                name: "孙燕姿".into(),
            }],
            album: Some(AlbumRef {
                id: hmp_core::AlbumId::new("a-1"),
                name: "孙燕姿经典全纪录".into(),
            }),
            duration: Some(std::time::Duration::from_secs(257)),
            cover: Some(CoverRef {
                url: "https://example.com/cover.jpg".into(),
            }),
            url: Some("https://example.com/stream.mp3".into()),
            available_qualities: vec![AudioQuality::Flac],
        }
    }

    #[test]
    fn status_strings_match_spec() {
        assert_eq!(
            playback_status(hmp_core::PlaybackStatus::Playing),
            "Playing"
        );
        assert_eq!(playback_status(hmp_core::PlaybackStatus::Paused), "Paused");
        assert_eq!(playback_status(hmp_core::PlaybackStatus::Ended), "Stopped");
        assert_eq!(loop_status(hmp_core::LoopMode::List), "Playlist");
        assert_eq!(loop_status(hmp_core::LoopMode::Track), "Track");
        assert_eq!(loop_status(hmp_core::LoopMode::None), "None");
    }

    #[test]
    fn metadata_contains_required_and_optional() {
        let t = track();
        let meta = metadata_from_track(&t);
        let map: std::collections::HashMap<_, _> = meta.into_iter().collect();

        assert!(map.contains_key("mpris:trackid"));
        assert!(map.contains_key("xesam:title"));
        assert!(map.contains_key("xesam:artist"));
        assert!(map.contains_key("xesam:albumArtist"));
        assert!(map.contains_key("xesam:album"));
        assert!(map.contains_key("mpris:artUrl"));
        assert!(map.contains_key("xesam:url"));
        assert_eq!(
            map["xesam:url"].downcast_ref::<String>(),
            Ok("https://example.com/stream.mp3".to_owned())
        );
        let album_artists = map["xesam:albumArtist"]
            .downcast_ref::<zvariant::Array>()
            .unwrap();
        assert_eq!(album_artists.len(), 1);
        assert_eq!(album_artists.get::<&str>(0).unwrap().unwrap(), "孙燕姿");
        assert_eq!(
            map["mpris:length"].downcast_ref::<u64>().unwrap(),
            257_000_000
        );
    }

    #[test]
    fn metadata_handles_minimal_track() {
        let t = Track::new(TrackId::new("empty"), "");
        let meta = metadata_from_track(&t);
        assert!(
            meta.iter().any(|(k, _)| *k == "mpris:trackid"),
            "trackid is always present"
        );
    }
}
