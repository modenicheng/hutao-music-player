//! Backend-neutral URI classification.

use std::path::PathBuf;
use std::sync::Arc;

use hmp_core::{HmpError, MediaStreamSource};
use url::Url;

/// A media location supported by the player.
///
/// 远端曲目不再经 HTTP 消费（历史上的回环解密代理/stream-download 路径已
/// 删除）：一律由 daemon 塞进 [`hmp_core::LoadRequest::stream`] 的进程内
/// [`MediaStreamSource`] 直连解码器，`uri` 仅保留作元数据/日志。
#[derive(Clone, Debug)]
pub enum MediaLocation {
    File(PathBuf),
    Stream(Arc<dyn MediaStreamSource>),
}

/// Decode a file URL using the platform's native path representation.
pub fn file_path(uri: &Url) -> Result<PathBuf, HmpError> {
    uri.to_file_path()
        .map_err(|()| HmpError::Playback(format!("invalid file URI: {uri}")))
}

/// Classify a player URI without performing I/O.
///
/// 只接受 `file://`：http/https 不是可播放地址，远端播放必须经
/// [`hmp_core::LoadRequest::stream`] 提供进程内 [`MediaStreamSource`]。
pub fn parse_uri(value: &str) -> Result<MediaLocation, HmpError> {
    let uri = Url::parse(value)
        .map_err(|error| HmpError::Playback(format!("invalid media URI {value:?}: {error}")))?;
    match uri.scheme() {
        "file" => file_path(&uri).map(MediaLocation::File),
        "http" | "https" => Err(HmpError::Playback(format!(
            "remote media URI {value:?} is not playable; pass an in-process \
             MediaStreamSource via LoadRequest::stream (HTTP playback removed)"
        ))),
        scheme => Err(HmpError::Playback(format!(
            "unsupported media URI scheme: {scheme}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_uri_scheme() {
        let error = parse_uri("ftp://example.invalid/song.flac").unwrap_err();
        assert!(error.to_string().contains("unsupported media URI scheme"));
    }

    /// http/https 必须给出指向 MediaStreamSource 的明确错误，
    /// 而不是静默退回某条已删除的 HTTP 路径。
    #[test]
    fn rejects_http_uri_pointing_to_stream_source() {
        let error = parse_uri("https://isure.stream.qqmusic.qq.com/song.flac").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("MediaStreamSource"), "{message}");
        assert!(message.contains("HTTP playback removed"), "{message}");
    }
}
