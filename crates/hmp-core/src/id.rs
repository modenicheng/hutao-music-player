//! 实体标识符 newtype（docs/PROJECT.md §7.2）。
//!
//! 避免把歌单 ID 误传给歌曲接口：`TrackId` 与 `AlbumId` 等类型互不兼容，
//! 但都与 `String` 可转换。

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// 构造标识符。
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
    };
}

id_type! {
    /// 歌曲标识符（如 QQ 音乐 `mid` 或数字 ID）。
    TrackId
}

id_type! {
    /// 专辑标识符。
    AlbumId
}

id_type! {
    /// 歌手标识符。
    ArtistId
}

id_type! {
    /// 歌单标识符。
    PlaylistId
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_are_distinct_types() {
        // 编译期类型安全：TrackId 不能直接用作 AlbumId
        let track = TrackId::new("003aQm4F3GJHZq");
        let album = AlbumId::new("003aQm4F3GJHZq");
        assert_eq!(track.to_string(), "003aQm4F3GJHZq");
        assert_eq!(album.as_ref(), "003aQm4F3GJHZq");
        assert_ne!(track, TrackId::new("other"));
    }

    #[test]
    fn ids_serialize_as_plain_string() {
        let id = PlaylistId::from("8655927861");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"8655927861\"");
        let back: PlaylistId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn ids_work_in_json_structures() {
        let v = json!({"track": TrackId::new("mid-1")});
        assert_eq!(v["track"], "mid-1");
    }

    #[test]
    fn empty_id_is_allowed_but_distinct() {
        assert_eq!(TrackId::new(""), TrackId::new(""));
        assert_ne!(TrackId::new(""), TrackId::new("x"));
    }
}
