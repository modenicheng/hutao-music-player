//! Backend-neutral URI classification.

use std::path::PathBuf;

use hmp_core::HmpError;
use url::Url;

/// A media location supported by the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaLocation {
    File(PathBuf),
    Http(Url),
}

/// Decode a file URL using the platform's native path representation.
pub fn file_path(uri: &Url) -> Result<PathBuf, HmpError> {
    uri.to_file_path()
        .map_err(|()| HmpError::Playback(format!("invalid file URI: {uri}")))
}

/// Classify a player URI without performing I/O.
pub fn parse_uri(value: &str) -> Result<MediaLocation, HmpError> {
    let uri = Url::parse(value)
        .map_err(|error| HmpError::Playback(format!("invalid media URI {value:?}: {error}")))?;
    match uri.scheme() {
        "file" => file_path(&uri).map(MediaLocation::File),
        "http" | "https" => Ok(MediaLocation::Http(uri)),
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
}
