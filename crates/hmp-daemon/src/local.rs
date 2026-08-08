//! 本地音乐解析器（媒体库重构 C2/C3）。
//!
//! 本地曲目身份 = `local:<绝对路径>`；播放 URI = `file://<path>`。
//! 解析即入库（幂等）：`add_local_file` upsert tracks + local_files，
//! 之后可按 `hmp play local:<path>` / `hmp library scan <dir>` 复用。
//! **不要求 QQ 登录**（登录门按 provider 判定，见 server.rs）。

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use hmp_core::{AlbumRef, ArtistId, ArtistRef, AudioQuality, PlayRequest, Track, TrackId};
use hmp_storage::LibraryDb;

use crate::player::EngineError;
use crate::player::{ResolvedTrack, SourceResolver};

/// 本地解析器（依赖媒体库）。
pub struct LocalSourceResolver {
    library: Arc<Mutex<LibraryDb>>,
}

impl std::fmt::Debug for LocalSourceResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSourceResolver").finish()
    }
}

impl LocalSourceResolver {
    pub fn new(library: Arc<Mutex<LibraryDb>>) -> Self {
        Self { library }
    }

    /// `local:<path>` → 路径。
    fn path_of(id: &TrackId) -> Result<&str, EngineError> {
        id.0.strip_prefix("local:")
            .filter(|p| !p.is_empty())
            .ok_or_else(|| EngineError::Internal(format!("非法本地曲目 id `{id}`")))
    }

    /// 按本地 id 解析（入库 + 构造 ResolvedTrack）。
    async fn resolve_local(&self, id: TrackId) -> Result<ResolvedTrack, EngineError> {
        let path = Self::path_of(&id)?;
        let path = Path::new(path);
        if !path.exists() {
            return Err(EngineError::TrackNotFound);
        }
        let meta = hmp_storage::read_meta(path);
        let title = meta
            .as_ref()
            .map(|m| m.title.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "未知曲目".to_string());
        let artist = meta.as_ref().and_then(|m| m.artist.clone());
        let album = meta.as_ref().and_then(|m| m.album.clone());
        let duration = meta.as_ref().and_then(|m| m.duration_ms);

        let uri = format!("file://{}", path.display());
        {
            let mut lib = self.library.lock().unwrap();
            lib.add_local_file(path, meta.as_ref())
                .map_err(|e| EngineError::Internal(format!("媒体库写入失败: {e}")))?;
        }
        let track = Track {
            id,
            title,
            artists: artist
                .map(|name| {
                    vec![ArtistRef {
                        id: ArtistId::new("local"),
                        name,
                    }]
                })
                .unwrap_or_default(),
            album: album.map(|name| AlbumRef {
                id: hmp_core::AlbumId::new("local"),
                name,
            }),
            duration: duration.map(|ms| std::time::Duration::from_millis(ms as u64)),
            cover: None,
            url: Some(uri.clone()),
            available_qualities: vec![AudioQuality::Mp3_128],
        };
        Ok(ResolvedTrack {
            track,
            uri,
            media: None,
            quality: AudioQuality::Mp3_128,
        })
    }
}

impl SourceResolver for LocalSourceResolver {
    fn resolve_source_ids(
        &self,
        src: &PlayRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TrackId>, EngineError>> + Send + '_>> {
        match src {
            PlayRequest::Local(id) => {
                let id = id.clone();
                Box::pin(async move { Ok(vec![id]) })
            }
            _ => Box::pin(async {
                Err(EngineError::Internal("本地解析器仅支持 local 源".into()))
            }),
        }
    }

    fn resolve_track(
        &self,
        id: &TrackId,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
        let id = id.clone();
        Box::pin(async move { self.resolve_local(id).await })
    }

    fn resolve_uri(
        &self,
        uri: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
        let uri = uri.to_string();
        Box::pin(async move {
            let Some(path) = uri.strip_prefix("file://") else {
                return Err(EngineError::Internal(format!(
                    "本地解析器不支持 URI `{uri}`"
                )));
            };
            self.resolve_local(TrackId::new(format!("local:{path}")))
                .await
        })
    }
}

/// 组合解析器：按 provider 分发（QQ / 本地）。
pub struct CompositeSourceResolver {
    qq: Arc<dyn SourceResolver>,
    local: Arc<dyn SourceResolver>,
}

impl std::fmt::Debug for CompositeSourceResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeSourceResolver")
            .field("qq", &self.qq)
            .field("local", &self.local)
            .finish()
    }
}

impl CompositeSourceResolver {
    pub fn new(qq: Arc<dyn SourceResolver>, local: Arc<dyn SourceResolver>) -> Self {
        Self { qq, local }
    }
}

impl SourceResolver for CompositeSourceResolver {
    fn resolve_source_ids(
        &self,
        src: &PlayRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TrackId>, EngineError>> + Send + '_>> {
        match src {
            PlayRequest::Local(_) => self.local.resolve_source_ids(src),
            _ => self.qq.resolve_source_ids(src),
        }
    }

    fn resolve_track(
        &self,
        id: &TrackId,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
        if hmp_core::TrackProvider::from_id(&id.0) == hmp_core::TrackProvider::Local {
            self.local.resolve_track(id)
        } else {
            self.qq.resolve_track(id)
        }
    }

    fn resolve_uri(
        &self,
        uri: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
        if uri.starts_with("file://") {
            self.local.resolve_uri(uri)
        } else {
            self.qq.resolve_uri(uri)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_of_parses_local_id() {
        assert_eq!(
            LocalSourceResolver::path_of(&TrackId::new("local:/a/b.mp3")).unwrap(),
            "/a/b.mp3"
        );
        assert!(LocalSourceResolver::path_of(&TrackId::new("mid123")).is_err());
        assert!(LocalSourceResolver::path_of(&TrackId::new("local:")).is_err());
    }

    /// 本地解析：无标签文件 → 文件名回退；入库后可再查；URI = file://。
    #[tokio::test]
    async fn resolve_local_file_falls_back_to_stem_and_ingests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("我的歌.mp3");
        std::fs::write(&path, b"not real audio, tag read fails").unwrap();

        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let resolver = LocalSourceResolver::new(lib.clone());
        let id = TrackId::new(format!("local:{}", path.display()));
        let resolved = resolver.resolve_track(&id).await.unwrap();

        assert_eq!(resolved.uri, format!("file://{}", path.display()));
        assert_eq!(resolved.track.title, "我的歌", "无标签应回退文件名");
        assert_eq!(resolved.quality, AudioQuality::Mp3_128);
        assert!(resolved.track.url.as_deref() == Some(resolved.uri.as_str()));

        // 已入库：可按 id 查询。
        let mut lib = lib.lock().unwrap();
        let db_id = lib.track_id("local", id.as_ref()).unwrap().unwrap();
        assert_eq!(
            lib.local_path(db_id).unwrap().unwrap(),
            path.display().to_string()
        );
    }

    /// 不存在的本地文件 → TrackNotFound。
    #[tokio::test]
    async fn resolve_missing_local_file_is_not_found() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let resolver = LocalSourceResolver::new(lib);
        let err = resolver
            .resolve_track(&TrackId::new("local:/nonexistent/x.mp3"))
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::TrackNotFound));
    }
}
