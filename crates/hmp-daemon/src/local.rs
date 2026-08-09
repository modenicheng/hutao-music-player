//! 本地音乐解析器（媒体库重构 C2/C3）。
//!
//! 本地曲目身份 = `local:<绝对路径>`；播放 URI = `file://<path>`。
//! 解析即入库（幂等）：`add_local_file` upsert tracks + local_files，
//! 之后可按 `hmp play local:<path>` / `hmp library scan <dir>` 复用。
//! **不要求 QQ 登录**（登录门按 provider 判定，见 server.rs）。

use std::future::Future;
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

    /// 本地 id 的路径规范化（相对路径/symlink → 绝对真实路径）。
    /// 保证 `local:<path>` 身份稳定：同一文件不因相对/绝对/symlink 生成多条记录（P1）。
    fn canonical_id(id: TrackId) -> TrackId {
        match id.0.strip_prefix("local:") {
            Some(p) if !p.is_empty() => match std::fs::canonicalize(p) {
                Ok(c) => TrackId::new(format!("local:{}", c.display())),
                Err(_) => id, // 不存在/不可达：保持原样，由 resolve_track 报 TrackNotFound
            },
            _ => id,
        }
    }

    /// 列表解析用的轻量 stub：canonicalize + 读文件元数据（与 `resolve_local`
    /// 同一提取逻辑；title 回退文件名）。供媒体库批量缓存与队列投影。
    fn local_stub(&self, id: &TrackId) -> hmp_core::TrackStub {
        let id = Self::canonical_id(id.clone());
        let (title, artists, album, duration_ms) = match Self::path_of(&id) {
            Ok(p) => {
                let meta = hmp_storage::read_meta(std::path::Path::new(p));
                let title = meta
                    .as_ref()
                    .map(|m| m.title.clone())
                    .filter(|t| !t.is_empty())
                    .or_else(|| {
                        std::path::Path::new(p)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| id.to_string());
                let artists = meta
                    .as_ref()
                    .and_then(|m| m.artist.clone())
                    .into_iter()
                    .collect::<Vec<_>>();
                let album = meta.as_ref().and_then(|m| m.album.clone());
                let duration_ms = meta.as_ref().and_then(|m| m.duration_ms);
                (title, artists, album, duration_ms)
            }
            Err(_) => (id.to_string(), Vec::new(), None, None),
        };
        hmp_core::TrackStub {
            id,
            title,
            artists,
            album,
            duration_ms,
        }
    }

    /// 按本地 id 解析（入库 + 构造 ResolvedTrack）。
    async fn resolve_local(&self, id: TrackId) -> Result<ResolvedTrack, EngineError> {
        let id = Self::canonical_id(id);
        let path = Self::path_of(&id)?;
        let path = std::fs::canonicalize(path).map_err(|_| EngineError::TrackNotFound)?;
        let meta = hmp_storage::read_meta(&path);
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

        // 文件 URI 走 URL 编码（空格/#/%/Unicode 安全，P1）。
        let uri = url::Url::from_file_path(&path)
            .map(|u| u.to_string())
            .map_err(|_| {
                EngineError::Internal(format!("路径无法编码为 file URI: {}", path.display()))
            })?;
        {
            let mut lib = self.library.lock().unwrap();
            lib.add_local_file(&path, meta.as_ref())
                .map_err(|e| EngineError::Internal(format!("媒体库写入失败: {e}")))?;
        }
        // 本地音质如实上报（按格式/码率；旧实现一律 Mp3_128，无损曲目被误报，P1）。
        let quality = local_quality(&path, meta.as_ref());
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
            available_qualities: vec![quality.clone()],
        };
        Ok(ResolvedTrack {
            track,
            uri,
            media: None,
            quality,
        })
    }
}

impl SourceResolver for LocalSourceResolver {
    fn resolve_source_ids(
        &self,
        src: &PlayRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>>
    {
        match src {
            PlayRequest::Local(id) => {
                let stub = self.local_stub(id);
                Box::pin(async move { Ok(vec![stub]) })
            }
            // 里程碑 E：`album:local:<专辑名>` → 本地专辑曲目列表（按名匹配）。
            PlayRequest::Album(id) if id.as_ref().starts_with("local:") => {
                let album = id.as_ref().trim_start_matches("local:").to_string();
                let lib = self.library.clone();
                Box::pin(async move {
                    let mut db = lib
                        .lock()
                        .map_err(|e| EngineError::Internal(e.to_string()))?;
                    let rows = db
                        .local_tracks_by_album(&album)
                        .map_err(|e| EngineError::Internal(e.to_string()))?;
                    if rows.is_empty() {
                        return Err(EngineError::PlaylistNotFound(format!(
                            "本地专辑为空: {album}"
                        )));
                    }
                    let stubs = rows
                        .into_iter()
                        .map(|r| hmp_core::TrackStub {
                            id: TrackId::new(r.source_key),
                            title: r.title,
                            artists: r.artist.into_iter().collect(),
                            album: r.album,
                            duration_ms: r.duration_ms,
                        })
                        .collect();
                    Ok(stubs)
                })
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
            // URL 解码（空格/#/%/Unicode 安全，P1）。
            let Ok(url) = url::Url::parse(&uri) else {
                return Err(EngineError::Internal(format!(
                    "本地解析器不支持 URI `{uri}`"
                )));
            };
            let Ok(path) = url.to_file_path() else {
                return Err(EngineError::Internal(format!(
                    "本地解析器不支持 URI `{uri}`"
                )));
            };
            self.resolve_local(TrackId::new(format!("local:{}", path.display())))
                .await
        })
    }
}

/// 本地文件音质如实映射（P1：旧实现一律 Mp3_128，FLAC/WAV 被误报）。
/// 无损格式 → Flac；AAC → Aac；OGG/Opus → Mp3_320（HQ 档）；MP3 按码率分档。
fn local_quality(path: &std::path::Path, meta: Option<&hmp_storage::LocalMeta>) -> AudioQuality {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let bitrate = meta.and_then(|m| m.bitrate);
    match ext.as_deref() {
        Some("flac" | "wav" | "ape") => AudioQuality::Flac,
        Some("m4a" | "aac") => AudioQuality::Aac,
        Some("ogg" | "opus") => AudioQuality::Mp3_320,
        Some("mp3") if bitrate.map(|b| b >= 300_000).unwrap_or(false) => AudioQuality::Mp3_320,
        _ => AudioQuality::Mp3_128,
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
    ) -> Pin<Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>>
    {
        match src {
            PlayRequest::Local(_) => self.local.resolve_source_ids(src),
            // 里程碑 E：本地专辑（`album:local:` 前缀）→ 本地解析器。
            PlayRequest::Album(id) if id.as_ref().starts_with("local:") => {
                self.local.resolve_source_ids(src)
            }
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

    /// 本地解析：无标签文件 → 文件名回退；入库后可再查；URI = file://（URL 编码）。
    #[tokio::test]
    async fn resolve_local_file_falls_back_to_stem_and_ingests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("我的歌.mp3");
        std::fs::write(&path, b"not real audio, tag read fails").unwrap();

        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let resolver = LocalSourceResolver::new(lib.clone());
        let canonical = std::fs::canonicalize(&path).unwrap();
        let id = TrackId::new(format!("local:{}", canonical.display()));
        let resolved = resolver.resolve_track(&id).await.unwrap();

        // URL 编码的 file URI（url crate）。
        let expected_uri = url::Url::from_file_path(&canonical).unwrap().to_string();
        assert_eq!(resolved.uri, expected_uri);
        assert_eq!(resolved.track.title, "我的歌", "无标签应回退文件名");
        assert!(resolved.track.url.as_deref() == Some(resolved.uri.as_str()));

        // 已入库：可按 canonical id 查询。
        let mut lib = lib.lock().unwrap();
        let db_id = lib.track_id("local", id.as_ref()).unwrap().unwrap();
        assert_eq!(
            lib.local_path(db_id).unwrap().unwrap(),
            canonical.display().to_string()
        );
    }

    /// P1：文件名含空格/`#`/Unicode 时，URI 必须正确编码并可往返解析。
    #[tokio::test]
    async fn resolve_uri_with_special_chars_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a b#c 我的歌.mp3");
        std::fs::write(&path, b"not real audio").unwrap();

        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let resolver = LocalSourceResolver::new(lib.clone());
        let uri = url::Url::from_file_path(std::fs::canonicalize(&path).unwrap())
            .unwrap()
            .to_string();
        assert!(uri.contains("%20"), "空格应编码: {uri}");
        let resolved = resolver.resolve_uri(&uri).await.unwrap();
        assert_eq!(resolved.uri, uri, "URI 应原样往返");
        assert_eq!(resolved.track.title, "a b#c 我的歌");
    }

    /// 里程碑 E：`album:local:<专辑名>` → 按专辑名取本地曲目列表（播放源）。
    #[tokio::test]
    async fn resolve_local_album_source() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        {
            let mut db = lib.lock().unwrap();
            for (path, title, album, artist, dur) in [
                ("/a.mp3", "A", "某专辑", "歌手1", 1000),
                ("/b.mp3", "B", "某专辑", "歌手1", 2000),
                ("/c.mp3", "C", "别的专辑", "歌手2", 3000),
            ] {
                db.add_local_file(std::path::Path::new(path), None).unwrap();
                db.upsert_track(&hmp_storage::TrackRow {
                    source: "local",
                    source_key: format!("local:{path}"),
                    title: title.into(),
                    album: Some(album.into()),
                    artist: Some(artist.into()),
                    duration_ms: Some(dur),
                    ..Default::default()
                })
                .unwrap();
            }
        }
        let resolver = LocalSourceResolver::new(lib);
        let src = PlayRequest::Album(hmp_core::AlbumId::new("local:某专辑"));
        let stubs = resolver.resolve_source_ids(&src).await.unwrap();
        assert_eq!(stubs.len(), 2, "专辑内 2 首");
        assert!(stubs.iter().all(|s| s.album.as_deref() == Some("某专辑")));
        assert!(stubs.iter().any(|s| s.title == "A"));
        assert!(stubs.iter().any(|s| s.title == "B"));
        // 空结果 → PlaylistNotFound。
        let src = PlayRequest::Album(hmp_core::AlbumId::new("local:不存在"));
        let err = resolver.resolve_source_ids(&src).await.unwrap_err();
        assert!(matches!(err, EngineError::PlaylistNotFound(_)));
    }

    /// P1：本地音质如实上报——flac → Flac（旧实现一律 Mp3_128）。
    #[tokio::test]
    async fn local_flac_reports_lossless_quality() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.flac");
        std::fs::write(&path, b"not real audio").unwrap();

        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let resolver = LocalSourceResolver::new(lib.clone());
        let id = TrackId::new(format!(
            "local:{}",
            std::fs::canonicalize(&path).unwrap().display()
        ));
        let resolved = resolver.resolve_track(&id).await.unwrap();
        assert_eq!(resolved.quality, AudioQuality::Flac, "FLAC 应报 Flac");
        assert_eq!(resolved.track.available_qualities, vec![AudioQuality::Flac]);
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
