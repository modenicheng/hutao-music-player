//! 本地音乐解析器（媒体库重构 C2/C3）。
//!
//! 本地曲目身份 = `local:<绝对路径>`；播放 URI = `file://<path>`。
//! 解析即入库（幂等）：`add_local_file` upsert tracks + local_files，
//! 之后可按 `hmp play local:<path>` / `hmp library scan <dir>` 复用。
//! **不要求 QQ 登录**（登录门按 provider 判定，见 server.rs）。

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use hmp_core::{
    AlbumId, AlbumRef, ArtistId, ArtistRef, AudioQuality, CoverRef, PlayRequest, Track, TrackId,
};
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
            .ok_or_else(|| EngineError::Internal(format!("invalid local track id `{id}`")))
    }

    /// 本地 id 的路径规范化（相对路径/symlink → 绝对真实路径）。
    /// 保证 `local:<path>` 身份稳定：同一文件不因相对/绝对/symlink 生成多条记录（P1）。
    fn canonical_id(id: TrackId) -> TrackId {
        match id.0.strip_prefix("local:") {
            // strip_verbatim：Windows canonicalize 产出 \?\ 前缀，须与库键统一拼写
            Some(p) if !p.is_empty() => match std::fs::canonicalize(p) {
                Ok(c) => TrackId::new(format!(
                    "local:{}",
                    hmp_storage::strip_verbatim(&c).display()
                )),
                Err(_) => id, // 不存在/不可达：保持原样，由 resolve_track 报 TrackNotFound
            },
            _ => id,
        }
    }

    /// 列表解析用的轻量 stub：canonicalize + 读文件元数据（与 `resolve_local`
    /// 同一提取逻辑；title 回退文件名）。供媒体库批量缓存与队列投影。
    /// 非常规文件（目录/设备/不存在）→ `None`：目录曾被当曲目 upsert 进库
    /// （垃圾行进 UI 列表、播放必败 5s 超时），此处源头拒绝。
    fn local_stub(&self, id: &TrackId) -> Option<hmp_core::TrackStub> {
        let id = Self::canonical_id(id.clone());
        let path = Self::path_of(&id).ok()?;
        let canonical = std::fs::canonicalize(path).ok()?;
        if !canonical.is_file() {
            return None;
        }
        let (title, artists, album, duration_ms) = {
            let meta = hmp_storage::read_meta(&canonical);
            let title = meta
                .as_ref()
                .map(|m| m.title.clone())
                .filter(|t| !t.is_empty())
                .or_else(|| {
                    canonical
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| id.to_string());
            // 多艺术家口径与扫描/播放路径一致（artists 全量；空则回退主歌手）。
            let artists: Vec<String> = meta
                .as_ref()
                .map(|m| {
                    if m.artists.is_empty() {
                        m.artist.iter().cloned().collect()
                    } else {
                        m.artists.clone()
                    }
                })
                .unwrap_or_default();
            let album = meta.as_ref().and_then(|m| m.album.clone());
            let duration_ms = meta.as_ref().and_then(|m| m.duration_ms);
            (title, artists, album, duration_ms)
        };
        Some(hmp_core::TrackStub {
            id,
            title,
            artists: artists.clone(),
            artist_refs: artists
                .iter()
                .map(|name| ArtistRef {
                    id: ArtistId::new("local"),
                    name: name.clone(),
                })
                .collect(),
            album,
            album_mid: None,
            duration_ms,
        })
    }

    /// 按本地 id 解析（入库 + 构造 ResolvedTrack）。
    async fn resolve_local(&self, id: TrackId) -> Result<ResolvedTrack, EngineError> {
        let id = Self::canonical_id(id);
        let path = Self::path_of(&id)?;
        let path = std::fs::canonicalize(path).map_err(|_| EngineError::TrackNotFound)?;
        // Windows canonicalize 产出 \?\ verbatim 前缀；入库/封面键/URI 同一
        // 规范拼写（strip 后仍是绝对真实路径，语义不变）。
        let path = hmp_storage::strip_verbatim(&path);
        // 目录/设备节点：Linux 上 File::open(dir) 成功而解码挂死，错误只会在
        // 引擎 5s 超时才浮出；源头拒绝 + 不入库（垃圾行防御）。
        if !path.is_file() {
            return Err(EngineError::TrackNotFound);
        }
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
            .unwrap_or_else(|| "(unknown title)".to_string());
        let artist_names: Vec<String> = meta
            .as_ref()
            .map(|m| {
                if m.artists.is_empty() {
                    m.artist.iter().cloned().collect()
                } else {
                    m.artists.clone()
                }
            })
            .unwrap_or_default();
        let album = meta.as_ref().and_then(|m| m.album.clone());
        let album_artist = meta.as_ref().and_then(|m| m.album_artist.clone());
        let duration = meta.as_ref().and_then(|m| m.duration_ms);
        let cover = project_local_cover(&path, meta.as_ref());

        // 文件 URI 走 URL 编码（空格/#/%/Unicode 安全，P1）。
        let uri = url::Url::from_file_path(&path)
            .map(|u| u.to_string())
            .map_err(|_| {
                EngineError::Internal(format!(
                    "path cannot be encoded as a file URI: {}",
                    path.display()
                ))
            })?;
        let (artist_ids, album_id) = {
            let mut lib = self.library.lock().unwrap();
            lib.add_local_file(&path, meta.as_ref())
                .map_err(|e| EngineError::Internal(format!("library write failed: {e}")))?;
            if let Some(cover) = &cover
                && let Err(error) =
                    lib.set_track_cover(&format!("local:{}", path.display()), &cover.url)
            {
                tracing::warn!(%error, path = %path.display(), "failed to index local cover");
            }
            // 实体 id：入库解析后的唯一身份（`track_artists.artist_id` /
            // `tracks.album_id`）。存储读投影没有行级 id 外读口，按实体聚合
            // 反查（实体名/专辑歌手唯一）；查不到 → None → "local" 哨兵。
            let artist_ids = if artist_names.is_empty() {
                Vec::new()
            } else {
                local_artist_entity_ids(&mut lib, &artist_names)
            };
            let album_id = album.as_deref().and_then(|title| {
                local_album_entity_id(
                    &mut lib,
                    title,
                    album_artist
                        .as_deref()
                        .or_else(|| artist_names.first().map(String::as_str)),
                )
            });
            (artist_ids, album_id)
        };
        // 本地音质如实上报（按格式/码率；旧实现一律 Mp3_128，无损曲目被误报，P1）。
        let quality = local_quality(&path, meta.as_ref());
        let track = Track {
            id,
            title,
            artists: artist_names
                .iter()
                .enumerate()
                .map(|(index, name)| ArtistRef {
                    id: local_artist_entity_link(artist_ids.get(index).copied().flatten()),
                    name: name.clone(),
                })
                .collect(),
            album: album.map(|name| AlbumRef {
                id: local_album_entity_link(album_id),
                name,
            }),
            duration: duration.map(|ms| std::time::Duration::from_millis(ms as u64)),
            cover,
            url: Some(uri.clone()),
            available_qualities: vec![quality.clone()],
        };
        Ok(ResolvedTrack {
            track,
            uri,
            media: None,
            quality,
            replaygain_db: meta.as_ref().and_then(|m| m.replaygain_track_db),
        })
    }
}

fn project_local_cover(
    track_path: &std::path::Path,
    meta: Option<&hmp_storage::LocalMeta>,
) -> Option<CoverRef> {
    meta.and_then(|meta| meta.cover.as_deref())
        .and_then(|bytes| match hmp_storage::scan::persist_cover(bytes) {
            Ok(url) => Some(CoverRef { url }),
            Err(error) => {
                tracing::warn!(%error, path = %track_path.display(), "failed to cache local cover");
                None
            }
        })
}

impl SourceResolver for LocalSourceResolver {
    fn resolve_source_ids(
        &self,
        src: &PlayRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>>
    {
        match src {
            PlayRequest::Local(id) => {
                // 非常规文件（目录/不存在）→ 空列表：调用方按「源解析为空」
                // 确定性失败，且 cache_stubs 不会把垃圾行写进库。
                let stubs: Vec<hmp_core::TrackStub> = self.local_stub(id).into_iter().collect();
                Box::pin(async move { Ok(stubs) })
            }
            // 媒体库/GUI 等通用入口可能以 `Track(local:...)` 表达本地单曲；
            // provider 由稳定 id 前缀识别，不能误送 QQ 解析器。
            PlayRequest::Track(id)
                if hmp_core::TrackProvider::from_id(id.as_ref())
                    == hmp_core::TrackProvider::Local =>
            {
                let stubs: Vec<hmp_core::TrackStub> = self.local_stub(id).into_iter().collect();
                Box::pin(async move { Ok(stubs) })
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
                            "local album is empty: {album}"
                        )));
                    }
                    let stubs = rows
                        .into_iter()
                        .map(|r| hmp_core::TrackStub {
                            id: TrackId::new(r.source_key),
                            title: r.title,
                            artists: r.artist.into_iter().collect(),
                            // 库内投影只有展示串，没有歌手 mid/列表；留空以免
                            // 用拼接串重写 track_artists（既有行保持不动）。
                            artist_refs: Vec::new(),
                            album: r.album,
                            album_mid: None,
                            duration_ms: r.duration_ms,
                        })
                        .collect();
                    Ok(stubs)
                })
            }
            // 里程碑 F：`playlist:local:<id>` → 本地歌单曲目（JOIN tracks，混排 QQ/本地）。
            PlayRequest::LibraryPlaylist(id) => {
                let lib = self.library.clone();
                let id = *id;
                Box::pin(async move {
                    let mut db = lib
                        .lock()
                        .map_err(|e| EngineError::Internal(e.to_string()))?;
                    let rows = db
                        .local_playlist_stubs(id)
                        .map_err(|e| EngineError::Internal(e.to_string()))?;
                    if rows.is_empty() {
                        return Err(EngineError::PlaylistNotFound(format!(
                            "local playlist is empty or missing: {id}"
                        )));
                    }
                    let stubs = rows
                        .into_iter()
                        .map(|r| hmp_core::TrackStub {
                            id: TrackId::new(r.source_key),
                            title: r.title,
                            artists: r.artist.into_iter().collect(),
                            // 库内投影只有展示串，没有歌手 mid/列表；留空以免
                            // 用拼接串重写 track_artists（既有行保持不动）。
                            artist_refs: Vec::new(),
                            album: r.album,
                            album_mid: None,
                            duration_ms: r.duration_ms,
                        })
                        .collect();
                    Ok(stubs)
                })
            }
            _ => Box::pin(async {
                Err(EngineError::Internal(
                    "local resolver only supports local sources".into(),
                ))
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
                    "local resolver does not support URI `{uri}`"
                )));
            };
            let Ok(path) = url.to_file_path() else {
                return Err(EngineError::Internal(format!(
                    "local resolver does not support URI `{uri}`"
                )));
            };
            self.resolve_local(TrackId::new(format!("local:{}", path.display())))
                .await
        })
    }
}

/// 本地实体导航 id 前缀（与桌面 `library_view` 的 `artist:`/`album:` 参数同
/// 契约，§30.2.6 phase 2b）：桌面 `player_bridge::detail_link_params` 直接
/// 透传为播放页链接参数。
const ARTIST_ID_PREFIX: &str = "artist:";
const ALBUM_ID_PREFIX: &str = "album:";

/// 本地歌手名 → 实体 id（`library_artists` 的实体投影；`NOCASE` 比较）。
/// 存储读投影没有行级 `artist_id`，按实体名反查；phase 2 跨 provider 名称
/// 归一后同名歌手只保留一个实体。
fn local_artist_entity_ids(db: &mut LibraryDb, names: &[String]) -> Vec<Option<i64>> {
    let Ok(groups) = db.library_artists() else {
        return vec![None; names.len()];
    };
    names
        .iter()
        .map(|name| {
            let key = name.trim().to_lowercase();
            groups
                .iter()
                .find(|group| group.id > 0 && group.artist.trim().to_lowercase() == key)
                .map(|group| group.id)
        })
        .collect()
}

/// 本地专辑（名 + 专辑歌手/首位歌手）→ 实体 id：同名不同专辑歌手只认歌手
/// 匹配的那个；无歌手匹配时标题唯一才认（否则回退 "local" 哨兵）。
fn local_album_entity_id(
    db: &mut LibraryDb,
    album: &str,
    key_artist: Option<&str>,
) -> Option<i64> {
    let title = album.trim().to_lowercase();
    if title.is_empty() {
        return None;
    }
    let groups = db.library_albums(Some(album)).ok()?;
    let same: Vec<_> = groups
        .iter()
        .filter(|group| group.id > 0 && group.album.trim().to_lowercase() == title)
        .collect();
    if let Some(named) = key_artist.map(str::trim).filter(|name| !name.is_empty()) {
        let named = named.to_lowercase();
        if let Some(group) = same.iter().find(|group| {
            group
                .artist
                .as_deref()
                .map(|artist| artist.trim().to_lowercase() == named)
                .unwrap_or(false)
        }) {
            return Some(group.id);
        }
    }
    if same.len() == 1 {
        return Some(same[0].id);
    }
    None
}

/// 本地歌手实体 id → `ArtistRef.id`（`artist:<id>`；无实体回退 `"local"`）。
fn local_artist_entity_link(entity_id: Option<i64>) -> ArtistId {
    match entity_id.filter(|id| *id > 0) {
        Some(id) => ArtistId::new(format!("{ARTIST_ID_PREFIX}{id}")),
        None => ArtistId::new("local"),
    }
}

/// 本地专辑实体 id → `AlbumRef.id`（`album:<id>`；无实体回退 `"local"`）。
fn local_album_entity_link(entity_id: Option<i64>) -> AlbumId {
    match entity_id.filter(|id| *id > 0) {
        Some(id) => AlbumId::new(format!("{ALBUM_ID_PREFIX}{id}")),
        None => AlbumId::new("local"),
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
        // lofty 的 audio_bitrate() 单位是 kbps（320 而非 320000）；
        // 旧阈值 300_000 恒为假，所有 MP3 都被误标为 Mp3_128。
        Some("mp3") if bitrate.map(|b| b >= 300).unwrap_or(false) => AudioQuality::Mp3_320,
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
        if src.is_local_source() {
            self.local.resolve_source_ids(src)
        } else {
            self.qq.resolve_source_ids(src)
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

    fn meta(title: &str, artist: &str, album: &str) -> hmp_storage::LocalMeta {
        hmp_storage::LocalMeta {
            title: title.into(),
            artist: Some(artist.into()),
            album: Some(album.into()),
            duration_ms: Some(1_000),
            ..Default::default()
        }
    }

    /// 本地实体 id 反查（§30.2.6 phase 2b）：歌手按实体名；专辑按（名, 歌手）
    /// 区分同名不同专辑歌手的两个实体；查不到 → None（回退 "local" 哨兵）。
    #[test]
    fn local_entity_ids_resolve_from_library() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.add_local_file(
            std::path::Path::new("/entity-a.flac"),
            Some(&meta("A", "周杰伦", "十一月的萧邦")),
        )
        .unwrap();
        db.add_local_file(
            std::path::Path::new("/entity-b.flac"),
            Some(&meta("B", "歌手乙", "十一月的萧邦")),
        )
        .unwrap();

        let artist_ids = local_artist_entity_ids(&mut db, &["周杰伦".into(), "查无此人".into()]);
        assert!(artist_ids[0].is_some_and(|id| id > 0));
        assert_eq!(artist_ids[1], None);

        let album_a = local_album_entity_id(&mut db, "十一月的萧邦", Some("周杰伦"));
        let album_b = local_album_entity_id(&mut db, "十一月的萧邦", Some("歌手乙"));
        assert!(album_a.is_some_and(|id| id > 0));
        assert!(album_b.is_some_and(|id| id > 0));
        assert_ne!(album_a, album_b, "同名不同专辑歌手是两个实体");
        assert_eq!(local_album_entity_id(&mut db, "不存在", None), None);

        assert_eq!(local_artist_entity_link(Some(7)).as_ref(), "artist:7");
        assert_eq!(local_artist_entity_link(None).as_ref(), "local");
        assert_eq!(local_album_entity_link(Some(9)).as_ref(), "album:9");
        assert_eq!(local_album_entity_link(None).as_ref(), "local");
    }

    #[tokio::test]
    async fn generic_track_with_local_id_resolves_as_local_source() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let resolver = LocalSourceResolver::new(lib);
        // 真实临时文件（目录/缺失路径如今被 is_file 守卫拒成空列表）。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.flac");
        std::fs::write(&path, b"not-a-real-flac-but-exists").unwrap();
        let source = PlayRequest::Track(TrackId::new(format!("local:{}", path.display())));

        let stubs = resolver.resolve_source_ids(&source).await.unwrap();

        assert_eq!(stubs.len(), 1);
        // 库键统一走 canonicalize + verbatim 剥离（Windows 上 canonicalize
        // 产出 \?\ 前缀，不能直接进键）。
        assert_eq!(
            stubs[0].id,
            TrackId::new(format!(
                "local:{}",
                hmp_storage::canonical_display_path(&path).display()
            ))
        );
    }

    #[tokio::test]
    async fn directory_and_missing_paths_resolve_to_no_tracks() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let resolver = LocalSourceResolver::new(lib);
        let dir = tempfile::tempdir().unwrap();
        // 目录：Linux File::open 能成功、解码挂死——解析期即拒（不入库）。
        let dir_src = PlayRequest::Local(TrackId::new(format!("local:{}", dir.path().display())));
        assert!(
            resolver
                .resolve_source_ids(&dir_src)
                .await
                .unwrap()
                .is_empty()
        );
        // 不存在路径：空列表（确定性失败，而非装载超时）。
        let missing_src = PlayRequest::Local(TrackId::new("local:/nonexistent/hmp-audit/x.flac"));
        assert!(
            resolver
                .resolve_source_ids(&missing_src)
                .await
                .unwrap()
                .is_empty()
        );
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
        // 库键走统一规范化（Windows canonicalize 的 \?\ 前缀不进键）。
        let key_path = hmp_storage::strip_verbatim(&canonical);
        let id = TrackId::new(format!("local:{}", key_path.display()));
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
            key_path.display().to_string()
        );
    }

    #[test]
    fn embedded_cover_projects_as_a_persisted_file_uri() {
        let meta = hmp_storage::LocalMeta {
            cover: Some(vec![1, 2, 3, 4]),
            ..Default::default()
        };
        let cover_url = project_local_cover(std::path::Path::new("fixture.flac"), Some(&meta))
            .expect("local playback must project the embedded cover")
            .url;
        let cover_path = url::Url::parse(&cover_url)
            .unwrap()
            .to_file_path()
            .expect("local covers must remain file URIs");
        assert!(cover_path.exists(), "persisted cover must be readable");
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

    /// 里程碑 F：`playlist:local:<id>` → 本地 SQLite 歌单曲目（混排 QQ/本地）。
    #[tokio::test]
    async fn resolve_library_playlist_source() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        {
            let mut db = lib.lock().unwrap();
            let pid = db.create_playlist("p").unwrap();
            db.add_playlist_track(pid, "qq", "mid-1", "QQ 歌").unwrap();
            db.add_playlist_track(pid, "local", "local:/x.mp3", "本地歌")
                .unwrap();
        }
        let resolver = LocalSourceResolver::new(lib);
        let src = hmp_core::PlayRequest::LibraryPlaylist(1);
        let stubs = resolver.resolve_source_ids(&src).await.unwrap();
        assert_eq!(stubs.len(), 2);
        assert_eq!(stubs[0].id.as_ref(), "mid-1");
        assert_eq!(stubs[1].id.as_ref(), "local:/x.mp3");
        assert_eq!(stubs[0].title, "QQ 歌");
    }

    #[tokio::test]
    async fn resolve_library_playlist_missing_is_not_found() {
        let lib = Arc::new(Mutex::new(LibraryDb::open_in_memory().unwrap()));
        let resolver = LocalSourceResolver::new(lib);
        let src = hmp_core::PlayRequest::LibraryPlaylist(999);
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
