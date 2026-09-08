//! 媒体库投影（M8 数据接线，页面侧）：直读 `hmp_storage::data_dir()/library.sqlite3`
//! （与 CLI 同契约，见 crates/hmp-cli/src/library.rs / history.rs）→ Data global 模型。
//! 播放/命令不在此处（daemon Unix socket IPC 另行接线）。
//!
//! 约束：
//! - 只读：桌面端对媒体库零写入（daemon 是唯一写方，WAL 允许跨进程并发读）；
//! - 离线诚实降级：库文件缺失/打开或投影失败 → 全空快照，不 panic、不回落 mock；
//! - 启动一次性静态快照：库变更推送属后端缺口，不做轮询。
//!
//! 已知投影缺口（storage 读 API 未暴露，诚实置空/置 0，详见交付报告）：
//! - `RecentPlay` 不带 source/source_key → QQ 历史行的播放键不可恢复（mid 置空）；
//! - 曲目读 API 不带 duration（`track_meta_batch`）→ QQ 行时长置 0；
//! - 本地行 cover_uri 只能经 `library_albums` 按专辑取（行级列未暴露）；
//! - local_files 的 file_size/format/mtime 列无读 API → 容量按现文件 stat 聚合，
//!   格式取源键路径扩展名（与 local_files.format 同源），last_scan 诚实显示 "—"。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use hmp_storage::db::{AlbumGroup, LibraryTrackRow};
use hmp_storage::{LibraryDb, TrackMeta};
use slint::Image;

use crate::TrackRow;
use crate::covers::cover_image;
use crate::format::{format_bytes, format_long_duration};

/// 最近播放页装载条数（页头统计与表格同一份；预览由桥侧截取前 5）。
const RECENT_LIMIT: u32 = 12;
/// 收藏列表上限（对齐 CLI `library tracks --liked` 的 10_000）。
const LIKED_LIMIT: u32 = 10_000;
/// 最近播放为空时页头时间的回退文案（与旧 mock 行为一致）。
const EMPTY_RECENT_TEXT: &str = "今天";
/// 扫描根无时间戳列（scan_roots 只有 generation）时的诚实占位。
const NO_TIMESTAMP_TEXT: &str = "—";

// ————————————————————————————————————————————————————————————
// 投影模型（纯数据，供桥侧转换为 slint struct；单测无需 UI 后端）
// ————————————————————————————————————————————————————————————

/// 曲目行（Data.TrackRow 的数据来源）。
#[derive(Clone, Debug)]
pub struct SongRow {
    /// 源键：QQ songmid / `local:<路径>`（TrackRow.mid，PlayRequest 映射用）。
    /// QQ 播放历史行可能不可恢复（storage 投影缺口）→ 空串。
    pub mid: String,
    /// 0=QQ 1=本地（TrackRow.source）。
    pub source: i32,
    pub title: String,
    /// 展示串（多人合唱以 " / " 连接；当前投影为单主艺术家）。
    pub artists: String,
    pub album: String,
    pub duration_ms: i32,
    /// 音质徽章文案；无损以下为空（对齐 mock 纪律）。
    pub quality: String,
    /// 本地封面 URI（`file://…`，扫描期 extract 落盘）；QQ http 封面不落 UI
    /// （项目原则禁 HTTP）→ None，渲染时走程序化封面。
    pub cover_uri: Option<String>,
    /// 命中的扫描根（本地行；监视文件夹过滤的分组归属，来自 root 前缀匹配）。
    pub folder: Option<String>,
}

/// 监视文件夹行（按扫描根聚合的曲目统计）。
#[derive(Clone, Debug)]
pub struct FolderStat {
    pub path: String,
    pub track_count: i32,
    /// 字节（现文件 stat 聚合；DB file_size 列无读 API，见模块注释）。
    pub size_bytes: u64,
    /// 无时间戳数据源 → 恒 "—"（不伪造）。
    pub last_scan: String,
}

/// 歌单卡（创建/收藏分组后）。
#[derive(Clone, Debug)]
pub struct PlaylistEntry {
    /// 歌单 DB id（十进制串，Nav 参数用）。
    pub id: String,
    pub name: String,
    /// 副标题 "N 首"——媒体库无播放计数，不伪造"X次播放"。
    pub subtitle: String,
}

/// 五个库页的一次性静态快照。
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub liked: Vec<SongRow>,
    pub created: Vec<PlaylistEntry>,
    pub favorited: Vec<PlaylistEntry>,
    pub recent: Vec<SongRow>,
    pub recent_latest: String,
    pub recent_earliest: String,
    pub local_tracks: Vec<SongRow>,
    pub folders: Vec<FolderStat>,
    /// 总时长/总容量文案（Rust 侧格式化，Slint 不做数字格式化）。
    pub local_duration_text: String,
    pub local_size_text: String,
}

impl Snapshot {
    /// 全空快照（离线诚实降级）。
    pub fn empty() -> Self {
        Self {
            liked: Vec::new(),
            created: Vec::new(),
            favorited: Vec::new(),
            recent: Vec::new(),
            recent_latest: EMPTY_RECENT_TEXT.into(),
            recent_earliest: EMPTY_RECENT_TEXT.into(),
            local_tracks: Vec::new(),
            folders: Vec::new(),
            local_duration_text: format_long_duration(0),
            local_size_text: format_bytes(0),
        }
    }
}

/// 装载快照：库文件缺失（未扫描/未播放过）→ 全空；打开/投影失败 → 记日志后全空。
/// 不主动建库文件（建库是 daemon/CLI 写方的职责，桌面端只读）。
pub fn load_snapshot() -> Snapshot {
    let path = hmp_storage::data_dir().join("library.sqlite3");
    if !path.exists() {
        return Snapshot::empty();
    }
    let mut db = match LibraryDb::open(&path) {
        Ok(db) => db,
        Err(err) => {
            tracing::warn!("媒体库打开失败，库页降级为空模型: {err}");
            return Snapshot::empty();
        }
    };
    read_snapshot(&mut db).unwrap_or_else(|| {
        tracing::warn!("媒体库投影失败，库页降级为空模型");
        Snapshot::empty()
    })
}

/// 详情页读取：打开库执行单次投影；库缺失/打开失败/投影失败 → None（调用方
/// 呈现"未找到"诚实空态）。导航时机调用（点击 → 装载），单页查询量小、同步读。
fn with_db<T>(read: impl FnOnce(&mut LibraryDb) -> Option<T>) -> Option<T> {
    let path = hmp_storage::data_dir().join("library.sqlite3");
    if !path.exists() {
        return None;
    }
    let mut db = LibraryDb::open(&path).ok()?;
    read(&mut db)
}

/// 单次投影：全部读查询走同一连接（进程内单连接 + WAL 跨进程并发读）。
/// 任一查询失败 → None（调用方降级全空；不部分渲染，避免页间口径漂移）。
fn read_snapshot(db: &mut LibraryDb) -> Option<Snapshot> {
    let album_covers = album_cover_map(db)?;
    let roots = db.scan_roots().ok()?;
    let local_rows_vec = db.library_tracks(None, None, None, false).ok()?;
    let local_by_key: HashMap<String, LibraryTrackRow> = local_rows_vec
        .iter()
        .map(|t| (t.source_key.clone(), t.clone()))
        .collect();

    let liked = liked_rows(db, &album_covers, &local_by_key)?;
    let (recent, recent_latest, recent_earliest) = recent_rows(db, &local_by_key, &album_covers)?;
    let (local_tracks, folders) = local_rows(&local_rows_vec, &album_covers, &roots)?;
    let (created, favorited) = playlist_entries(db)?;

    let total_ms: u64 = local_tracks
        .iter()
        .map(|row| row.duration_ms.max(0) as u64)
        .sum();
    let total_size: u64 = folders.iter().map(|f| f.size_bytes).sum();
    Some(Snapshot {
        liked,
        created,
        favorited,
        recent,
        recent_latest,
        recent_earliest,
        local_tracks,
        folders,
        local_duration_text: format_long_duration(total_ms),
        local_size_text: format_bytes(total_size),
    })
}

// ————————————————————————————————————————————————————————————
// 各页投影
// ————————————————————————————————————————————————————————————

/// 专辑名 → 封面 URI（`library_albums` 的 MAX(cover_uri)；扫描为整专辑提取
/// 同源内嵌图，专辑级封面即行级封面的忠实代理）。
fn album_cover_map(db: &mut LibraryDb) -> Option<HashMap<String, String>> {
    Some(
        db.library_albums(None)
            .ok()?
            .into_iter()
            .filter_map(|a| a.cover_uri.map(|uri| (a.album, uri)))
            .collect(),
    )
}

/// 我喜欢：`list_favorites`（本地事实视图，新→旧）为权威序；
/// 本地行经全量本地曲目表补全元数据，QQ 行经 `track_meta_batch` 补全歌手/专辑。
fn liked_rows(
    db: &mut LibraryDb,
    album_covers: &HashMap<String, String>,
    local_by_key: &HashMap<String, LibraryTrackRow>,
) -> Option<Vec<SongRow>> {
    let favs = db.list_favorites(LIKED_LIMIT).ok()?;
    let qq_meta = qq_meta_map(db, favs.iter().map(|f| f.source_key.clone()).collect());

    Some(
        favs.into_iter()
            .map(|f| match f.source.as_str() {
                "local" => match local_by_key.get(f.source_key.as_str()) {
                    Some(t) => SongRow {
                        mid: t.source_key.clone(),
                        source: 1,
                        title: t.title.clone(),
                        artists: t.artist.clone().unwrap_or_default(),
                        album: t.album.clone().unwrap_or_default(),
                        duration_ms: t.duration_ms.unwrap_or(0) as i32,
                        quality: quality_text(format_of_key(&t.source_key)),
                        cover_uri: t
                            .album
                            .as_deref()
                            .and_then(|album| album_covers.get(album).cloned()),
                        folder: None,
                    },
                    // 收藏先于扫描入库（无 local_files 行）：稀疏行诚实展示。
                    None => SongRow {
                        mid: f.source_key.clone(),
                        source: 1,
                        title: f.title,
                        artists: String::new(),
                        album: String::new(),
                        duration_ms: 0,
                        quality: quality_text(format_of_key(&f.source_key)),
                        cover_uri: None,
                        folder: None,
                    },
                },
                // QQ 行：无时长/音质/封面读 API（http 封面 UI 禁网）→ 程序化占位。
                _ => song_row_from_qq(&f.source_key, f.title, qq_meta.get(f.source_key.as_str())),
            })
            .collect(),
    )
}

/// 最近播放：`recent_tracks` LRU 视图（一曲一行，再播置顶——同曲重复播放
/// 不再产生重复行/重复高亮）；latest/earliest 取首尾真实时间戳。
/// 播放键随投影下发（source/source_key）：本地行经本地曲目表补全时长/专辑/封面，
/// QQ 行经 `track_meta_batch` 补全歌手/专辑（与我喜欢页同口径，mid 可直接回放）。
fn recent_rows(
    db: &mut LibraryDb,
    local_by_key: &HashMap<String, LibraryTrackRow>,
    album_covers: &HashMap<String, String>,
) -> Option<(Vec<SongRow>, String, String)> {
    let plays = db.recent_tracks(RECENT_LIMIT).ok()?;
    if plays.is_empty() {
        return Some((
            Vec::new(),
            EMPTY_RECENT_TEXT.into(),
            EMPTY_RECENT_TEXT.into(),
        ));
    }
    let latest = format_stamp(plays.first().map(|p| p.started_at).unwrap_or_default());
    let earliest = format_stamp(plays.last().map(|p| p.started_at).unwrap_or_default());
    let qq_meta = qq_meta_map(db, plays.iter().map(|p| p.source_key.clone()).collect());
    let rows = plays
        .into_iter()
        .map(|play| match play.source.as_str() {
            "local" => {
                let key = play.source_key;
                match local_by_key.get(key.as_str()) {
                    Some(t) => SongRow {
                        mid: key,
                        source: 1,
                        title: play.title,
                        artists: play.artist.unwrap_or_default(),
                        album: t.album.clone().unwrap_or_default(),
                        duration_ms: t.duration_ms.unwrap_or(0) as i32,
                        quality: quality_text(format_of_key(&t.source_key)),
                        cover_uri: t
                            .album
                            .as_deref()
                            .and_then(|album| album_covers.get(album).cloned()),
                        folder: None,
                    },
                    None => {
                        let quality = quality_text(format_of_key(&key));
                        SongRow {
                            mid: key,
                            source: 1,
                            title: play.title,
                            artists: play.artist.unwrap_or_default(),
                            album: String::new(),
                            duration_ms: 0,
                            quality,
                            cover_uri: None,
                            folder: None,
                        }
                    }
                }
            }
            // QQ 行：mid 即播放键（此前 RecentPlay 不带 source/source_key，
            // 只能置空 → 整表播放必败）；元数据走库内缓存批量补全。
            _ => song_row_from_qq(
                &play.source_key,
                play.title,
                qq_meta.get(play.source_key.as_str()),
            ),
        })
        .collect();
    Some((rows, latest, earliest))
}

/// 音乐库：全部本地曲目行（library_tracks ORDER BY title）+ 按扫描根前缀
/// 分组的文件夹统计（嵌套根取最长前缀，与 `scan_root_for` 同语义）。
fn local_rows(
    rows: &[LibraryTrackRow],
    album_covers: &HashMap<String, String>,
    roots: &[String],
) -> Option<(Vec<SongRow>, Vec<FolderStat>)> {
    // 根序 = scan_roots 的 id 序（注册序），稳定展示。
    let mut counts = vec![0i32; roots.len()];
    let mut sizes = vec![0u64; roots.len()];

    let songs = rows
        .iter()
        .map(|t| {
            let path = t.source_key.strip_prefix("local:").map(Path::new);
            let root_idx = path.and_then(|p| root_index(roots, p));
            if let (Some(i), Some(p)) = (root_idx, path) {
                counts[i] += 1;
                // DB file_size 列无读 API → stat 现文件（缺失/离线盘贡献 0）。
                sizes[i] += std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            }
            SongRow {
                mid: t.source_key.clone(),
                source: 1,
                title: t.title.clone(),
                artists: t.artist.clone().unwrap_or_default(),
                album: t.album.clone().unwrap_or_default(),
                duration_ms: t.duration_ms.unwrap_or(0) as i32,
                quality: quality_text(format_of_key(&t.source_key)),
                cover_uri: t
                    .album
                    .as_deref()
                    .and_then(|album| album_covers.get(album).cloned()),
                folder: root_idx.map(|i| roots[i].clone()),
            }
        })
        .collect();
    let folders = roots
        .iter()
        .zip(counts)
        .zip(sizes)
        .map(|((path, track_count), size_bytes)| FolderStat {
            path: path.clone(),
            track_count,
            size_bytes,
            last_scan: NO_TIMESTAMP_TEXT.into(),
        })
        .collect();
    Some((songs, folders))
}

/// 歌单按 relation 分流：local/owned → 自建组，subscribed → 收藏组
/// （owned pending 删除的行也在列表，与 CLI `playlist list` 同口径）。
fn playlist_entries(db: &mut LibraryDb) -> Option<(Vec<PlaylistEntry>, Vec<PlaylistEntry>)> {
    let mut created = Vec::new();
    let mut favorited = Vec::new();
    for p in db.list_playlists().ok()? {
        let entry = PlaylistEntry {
            id: p.id.to_string(),
            name: p.name,
            subtitle: format!("{} 首", p.track_count),
        };
        if p.relation == "subscribed" {
            favorited.push(entry);
        } else {
            created.push(entry);
        }
    }
    Some((created, favorited))
}

// ————————————————————————————————————————————————————————————
// 详情页投影（歌单/专辑/歌手；本地媒体库为源，远端 mid 等内容接口补齐前
// 详情页参数 = 展示名，见 PORTING.md M8.2）
// ————————————————————————————————————————————————————————————

/// 歌单详情（PlaylistView.vue 的本地投影：创建者/标签/简介/播放数无数据源，
/// 头部只呈 N 首 + 总时长）。
#[derive(Clone, Debug)]
pub struct PlaylistDetail {
    pub name: String,
    pub tracks: Vec<SongRow>,
    pub total_ms: u64,
}

/// 专辑详情（AlbumView.vue 的本地投影：发行厂牌/简介/收藏计数无数据源）。
#[derive(Clone, Debug)]
pub struct AlbumDetail {
    pub name: String,
    pub artist: Option<String>,
    pub year: Option<i64>,
    pub cover_uri: Option<String>,
    pub tracks: Vec<SongRow>,
    pub total_ms: u64,
}

/// 歌手页专辑卡（Vue 的 releaseDate 副标题；无年份诚实显示 "—"）。
#[derive(Clone, Debug)]
pub struct ArtistAlbum {
    pub name: String,
    pub year_text: String,
    pub track_count: i32,
    pub cover_uri: Option<String>,
}

/// 歌手详情（ArtistView.vue 的本地投影：照片/简介/MV/相似歌手无数据源，
/// hero 只呈统计带；name 是库内规范名，可能与入参展示串不同）。
#[derive(Clone, Debug)]
pub struct ArtistDetail {
    pub name: String,
    pub tracks: Vec<SongRow>,
    pub albums: Vec<ArtistAlbum>,
}

/// 歌单详情：曲目行按 position 序；本地行经本地曲目表补全，QQ 行经
/// `track_meta_batch` 补全歌手/专辑（时长无读 API → 0，同收藏页口径）。
pub fn playlist_detail(id: i64) -> Option<PlaylistDetail> {
    with_db(|db| playlist_detail_from(db, id))
}

fn playlist_detail_from(db: &mut LibraryDb, id: i64) -> Option<PlaylistDetail> {
    let name = db
        .list_playlists()
        .ok()?
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| p.name)?;
    let rows = db.playlist_tracks(id).ok()?;
    let local_by_key = local_track_map(db)?;
    let album_covers = album_cover_map(db)?;
    let qq_meta = qq_meta_map(db, rows.iter().map(|r| r.source_key.clone()).collect());

    let tracks: Vec<SongRow> = rows
        .iter()
        .map(|row| match row.source_key.strip_prefix("local:") {
            Some(_) => match local_by_key.get(row.source_key.as_str()) {
                Some(t) => song_row_from_local(t, &album_covers),
                // 本地曲目已被移出扫描（收藏/歌单行仍在）：稀疏行诚实展示。
                None => SongRow {
                    mid: row.source_key.clone(),
                    source: 1,
                    title: row.title.clone(),
                    artists: String::new(),
                    album: String::new(),
                    duration_ms: 0,
                    quality: quality_text(format_of_key(&row.source_key)),
                    cover_uri: None,
                    folder: None,
                },
            },
            None => song_row_from_qq(
                &row.source_key,
                row.title.clone(),
                qq_meta.get(&row.source_key),
            ),
        })
        .collect();
    let total_ms: u64 = tracks.iter().map(|r| r.duration_ms.max(0) as u64).sum();
    Some(PlaylistDetail {
        name,
        tracks,
        total_ms,
    })
}

/// 专辑详情：按专辑名（大小写不敏感精确）取本地曲目（track_number 序）；
/// 头部元数据（主歌手/年份/封面）取 `library_albums` 组行。
pub fn album_detail(name: &str) -> Option<AlbumDetail> {
    with_db(|db| album_detail_from(db, name))
}

fn album_detail_from(db: &mut LibraryDb, name: &str) -> Option<AlbumDetail> {
    // search 是子串过滤：组行须再按名精确（NOCASE）命中，避免"Love Story"
    // 命中 "Love Stories"。
    let group = db
        .library_albums(Some(name))
        .ok()?
        .into_iter()
        .find(|a| a.album.eq_ignore_ascii_case(name))?;
    let track_rows = db.local_tracks_by_album(name).ok()?;
    let mut album_covers = HashMap::new();
    if let Some(uri) = group.cover_uri.clone() {
        album_covers.insert(group.album.clone(), uri);
    }
    let tracks: Vec<SongRow> = track_rows
        .iter()
        .map(|t| song_row_from_local(t, &album_covers))
        .collect();
    let total_ms: u64 = tracks.iter().map(|r| r.duration_ms.max(0) as u64).sum();
    Some(AlbumDetail {
        name: group.album,
        artist: group.artist,
        year: group.year,
        cover_uri: group.cover_uri,
        tracks,
        total_ms,
    })
}

/// 歌手详情：曲目按 track_artists 精确名命中；入参是曲目行展示串（主歌手
/// 原始标签，可能含分隔符），与库内歌手名不一致时按最长包含匹配归一。
/// 专辑 = 该歌手曲目出现过的专辑（组行提供年份/封面/全碟曲目数）。
pub fn artist_detail(name: &str) -> Option<ArtistDetail> {
    with_db(|db| artist_detail_from(db, name))
}

fn artist_detail_from(db: &mut LibraryDb, param: &str) -> Option<ArtistDetail> {
    if param.is_empty() {
        return None;
    }
    let mut name = param.to_string();
    let mut tracks = db.local_tracks_by_artist(param).ok()?;
    if tracks.is_empty() {
        let resolved = db
            .library_artists()
            .ok()?
            .into_iter()
            .map(|a| a.artist)
            .filter(|canonical| param.contains(canonical.as_str()))
            .max_by_key(|canonical| canonical.len());
        if let Some(canonical) = resolved {
            name = canonical;
            tracks = db.local_tracks_by_artist(&name).ok()?;
        }
    }

    let album_covers = album_cover_map(db)?;
    let groups: HashMap<String, AlbumGroup> = db
        .library_albums(None)
        .ok()?
        .into_iter()
        .map(|g| (g.album.clone(), g))
        .collect();
    let mut albums: Vec<ArtistAlbum> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in &tracks {
        let Some(album) = t.album.as_deref() else {
            continue;
        };
        if !seen.insert(album.to_string()) {
            continue;
        }
        if let Some(g) = groups.get(album) {
            albums.push(ArtistAlbum {
                name: g.album.clone(),
                year_text: g.year.map(|y| y.to_string()).unwrap_or_else(|| "—".into()),
                track_count: g.track_count as i32,
                cover_uri: g.cover_uri.clone(),
            });
        }
    }
    // 年份降序、缺年份垫底，同年按名稳定序（发行时间线阅读顺序）。
    albums.sort_by(|a, b| {
        let year_of = |text: &str| {
            if text == "—" {
                i64::MIN
            } else {
                text.parse().unwrap_or(i64::MIN)
            }
        };
        year_of(&b.year_text)
            .cmp(&year_of(&a.year_text))
            .then_with(|| a.name.cmp(&b.name))
    });
    let cover_map = album_covers;
    let track_rows: Vec<SongRow> = tracks
        .iter()
        .map(|t| song_row_from_local(t, &cover_map))
        .collect();
    Some(ArtistDetail {
        name,
        tracks: track_rows,
        albums,
    })
}

// ————————————————————————————————————————————————————————————
// 助手
// ————————————————————————————————————————————————————————————

/// 本地全量曲目表（source_key → 行）：详情投影的行级元数据补全源。
fn local_track_map(db: &mut LibraryDb) -> Option<HashMap<String, LibraryTrackRow>> {
    Some(
        db.library_tracks(None, None, None, false)
            .ok()?
            .into_iter()
            .map(|t| (t.source_key.clone(), t))
            .collect(),
    )
}

/// QQ 曲目元数据（source_key → TrackMeta）：详情投影的歌手/专辑补全源。
fn qq_meta_map(db: &mut LibraryDb, keys: Vec<String>) -> HashMap<String, TrackMeta> {
    let qq_keys: Vec<String> = keys
        .into_iter()
        .filter(|key| !key.starts_with("local:"))
        .collect();
    db.track_meta_batch("qq", &qq_keys)
        .unwrap_or_default()
        .into_iter()
        .map(|m| (m.source_key.clone(), m))
        .collect()
}

/// 本地行 → SongRow（详情投影与收藏页共用口径：时长/音质来自本地曲目表，
/// 封面按专辑聚合命中）。
fn song_row_from_local(t: &LibraryTrackRow, album_covers: &HashMap<String, String>) -> SongRow {
    SongRow {
        mid: t.source_key.clone(),
        source: 1,
        title: t.title.clone(),
        artists: t.artist.clone().unwrap_or_default(),
        album: t.album.clone().unwrap_or_default(),
        duration_ms: t.duration_ms.unwrap_or(0) as i32,
        quality: quality_text(format_of_key(&t.source_key)),
        cover_uri: t
            .album
            .as_deref()
            .and_then(|album| album_covers.get(album).cloned()),
        folder: None,
    }
}

/// QQ 行 → SongRow：无时长/音质/封面读 API（http 封面 UI 禁网）→ 诚实置空。
fn song_row_from_qq(source_key: &str, title: String, meta: Option<&TrackMeta>) -> SongRow {
    SongRow {
        mid: source_key.to_string(),
        source: 0,
        title,
        artists: meta.and_then(|m| m.artist.clone()).unwrap_or_default(),
        album: meta.and_then(|m| m.album.clone()).unwrap_or_default(),
        duration_ms: 0,
        quality: String::new(),
        cover_uri: None,
        folder: None,
    }
}

/// 路径所属扫描根下标（组件级前缀 + 最长优先；与 `LibraryDb::scan_root_for` 一致）。
fn root_index(roots: &[String], path: &Path) -> Option<usize> {
    roots
        .iter()
        .enumerate()
        .filter(|(_, root)| path.starts_with(Path::new(root.as_str())))
        .max_by_key(|(_, root)| root.len())
        .map(|(i, _)| i)
}

/// 本地格式 = 源键路径扩展名（与 local_files.format 同源：local.rs read_meta
/// 存的就是小写扩展名）。
fn format_of_key(source_key: &str) -> Option<&str> {
    Path::new(source_key.strip_prefix("local:")?)
        .extension()
        .and_then(|e| e.to_str())
}

/// 音质徽章文案：FLAC→"FLAC"、HiRes→"Hi-Res"、320→"320kbps MP3"，无则空串。
/// 对齐 mock 的"无损以下为空"纪律：本地行只有扩展名证据（mp3 等有损格式
/// 无码率数据，不标 320）；QQ 行无质量数据 → 空串。
pub fn quality_text(format: Option<&str>) -> String {
    match format.map(|f| f.to_ascii_lowercase()).as_deref() {
        Some("flac") => "FLAC".into(),
        Some("hires") => "Hi-Res".into(),
        Some("320") => "320kbps MP3".into(),
        _ => String::new(),
    }
}

/// unix 秒 → "MM-DD HH:mm"（UTC；Howard Hinnant 民用历法，与 hmp-cli
/// timefmt 同款算法。桌面 crate 无 chrono/本地时区源，UTC 与 CLI `history`
/// 输出口径一致）。
pub fn format_stamp(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let rem = ts.rem_euclid(86_400);
    let (h, m) = (rem / 3600, (rem % 3600) / 60);
    let (_y, mo, d) = civil_from_days(days);
    format!("{mo:02}-{d:02} {h:02}:{m:02}")
}

/// 天数（自 1970-01-01）→ (年, 月, 日)。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

impl SongRow {
    /// SongRow → TrackRow。封面分流：`file://` 本地封面读盘（按路径缓存，
    /// 失败回退程序化封面）；QQ http 封面/无封面 → 程序化占位（seed = 源键，
    /// 缺键回退标题，保证同实体各处同图、UI 零 HTTP）。
    pub fn to_track_row(&self) -> TrackRow {
        let seed = if self.mid.is_empty() {
            &self.title
        } else {
            &self.mid
        };
        let cover =
            local_cover_image(self.cover_uri.as_deref()).unwrap_or_else(|| cover_image(seed));
        TrackRow {
            mid: self.mid.clone().into(),
            source: self.source,
            title: self.title.clone().into(),
            artists: self.artists.clone().into(),
            // 详情页参数：远端 mid 要等内容接口（AUDIT §8.2），本地投影期用
            // 展示名（歌手/专辑页按名查库）；无数据 → 空串（行链接禁用）。
            artist_mid: self.artists.clone().into(),
            album: self.album.clone().into(),
            album_mid: self.album.clone().into(),
            duration_ms: self.duration_ms,
            quality: self.quality.clone().into(),
            cover,
        }
    }
}

/// 本地封面读盘（`file://` URI）。缓存按 URI（即路径）去重——同一专辑封面
/// 被多条曲目复用。注意 slint::Image 非 Send/Sync（covers.rs 同款约束），
/// `Mutex<HashMap>` 编译不过 → thread_local；装载与 UI 消费同在主线程。
pub fn local_cover_image(uri: Option<&str>) -> Option<Image> {
    let uri = uri?;
    if !uri.starts_with("file://") {
        return None; // QQ http 封面：项目原则禁 HTTP，不绕
    }
    thread_local! {
        static CACHE: RefCell<HashMap<String, Image>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        if let Some(hit) = cache.borrow().get(uri) {
            return Some(hit.clone());
        }
        let image = Image::load_from_path(Path::new(uri.strip_prefix("file://")?)).ok()?;
        cache.borrow_mut().insert(uri.to_owned(), image.clone());
        Some(image)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmp_storage::{LocalMeta, PlayEnd};

    /// 测试根目录（storage 同款：temp_dir + 进程号，不引 tempfile dev-dep）。
    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hmp-lv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn meta(title: &str, artist: &str, album: &str, duration_ms: i64, format: &str) -> LocalMeta {
        LocalMeta {
            title: title.into(),
            artist: Some(artist.into()),
            album: Some(album.into()),
            duration_ms: Some(duration_ms),
            format: Some(format.into()),
            ..Default::default()
        }
    }

    #[test]
    fn quality_text_mapping() {
        assert_eq!(quality_text(Some("flac")), "FLAC");
        assert_eq!(quality_text(Some("FLAC")), "FLAC");
        assert_eq!(quality_text(Some("hires")), "Hi-Res");
        assert_eq!(quality_text(Some("320")), "320kbps MP3");
        // 有损格式无码率证据 → 空（无损以下为空纪律）
        assert_eq!(quality_text(Some("mp3")), "");
        assert_eq!(quality_text(None), "");
    }

    #[test]
    fn stamp_formatting() {
        // 1970-01-01 00:00 UTC
        assert_eq!(format_stamp(0), "01-01 00:00");
        // 2026-08-08 12:00 UTC（CLI history 测试同款参考值）
        assert_eq!(format_stamp(1_786_190_400), "08-08 12:00");
    }

    #[test]
    fn empty_db_projects_empty_snapshot() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let snap = read_snapshot(&mut db).expect("projection ok");
        assert!(snap.liked.is_empty());
        assert!(snap.created.is_empty());
        assert!(snap.favorited.is_empty());
        assert!(snap.recent.is_empty());
        assert!(snap.local_tracks.is_empty());
        assert!(snap.folders.is_empty());
        assert_eq!(snap.recent_latest, "今天");
        assert_eq!(snap.recent_earliest, "今天");
        assert_eq!(snap.local_duration_text, "0 分钟");
        assert_eq!(snap.local_size_text, "0 MB");
    }

    /// 我喜欢投影：本地行带时长/音质/专辑封面，QQ 行经 meta 补全歌手/专辑、
    /// 时长诚实置 0（无读 API）。
    #[test]
    fn liked_rows_enrich_local_and_qq() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = test_dir("liked");
        let a = dir.join("a.flac");
        let key_a = format!("local:{}", a.display());
        db.add_local_file(
            &a,
            Some(&meta("夜曲", "周杰伦", "十一月的萧邦", 180_000, "flac")),
        )
        .unwrap();
        db.add_favorite("local", &key_a, "夜曲").unwrap();
        // 扫描期封面落盘（同专辑任一行带封面即可经专辑聚合命中）。
        db.set_track_cover(&key_a, "file:///covers/x.jpg").unwrap();

        // QQ 收藏：先 upsert 元数据再收藏（模拟 daemon 列表缓存 + 收藏）。
        db.upsert_track(&hmp_storage::TrackRow {
            source: "qq",
            source_key: "mid-9".into(),
            title: "晴天".into(),
            artist: Some("周杰伦".into()),
            album: Some("叶惠美".into()),
            duration_ms: Some(269_000),
            ..Default::default()
        })
        .unwrap();
        db.add_favorite("qq", "mid-9", "晴天").unwrap();

        let snap = read_snapshot(&mut db).expect("projection ok");
        assert_eq!(snap.liked.len(), 2);

        let local = snap.liked.iter().find(|r| r.source == 1).unwrap();
        assert_eq!(local.mid, key_a);
        assert_eq!(local.title, "夜曲");
        assert_eq!(local.artists, "周杰伦");
        assert_eq!(local.album, "十一月的萧邦");
        assert_eq!(local.duration_ms, 180_000);
        assert_eq!(local.quality, "FLAC");
        assert_eq!(local.cover_uri.as_deref(), Some("file:///covers/x.jpg"));

        let qq = snap.liked.iter().find(|r| r.source == 0).unwrap();
        assert_eq!(qq.mid, "mid-9");
        assert_eq!(qq.artists, "周杰伦");
        assert_eq!(qq.album, "叶惠美");
        assert_eq!(qq.duration_ms, 0, "QQ 行时长无读 API，诚实置 0");
        assert_eq!(qq.quality, "");
    }

    /// 最近播放投影：latest/earliest 用真实时间戳格式化；本地行补全时长；
    /// 同曲两次播放收敛为一行（LRU 视图，不产生重复行/重复高亮）。
    #[test]
    fn recent_rows_format_latest_and_earliest() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = test_dir("recent");
        let f = dir.join("played.flac");
        db.add_local_file(
            &f,
            Some(&meta("被播放的歌", "歌手", "专辑", 200_000, "flac")),
        )
        .unwrap();
        let key = format!("local:{}", f.display());
        let id = db.track_id("local", &key).unwrap().unwrap();
        // 两次播放同一曲：LRU 去重后仅一行，时间戳取最近一次会话。
        let ev = db.record_play_start(id, 1_785_000_000).unwrap(); // 2026-07-25 17:20 UTC
        db.record_play_end(
            ev,
            &PlayEnd {
                track_id: id,
                ended_at: 1_785_000_600,
                listened_ms: 90_000,
                reason: "ended",
            },
        )
        .unwrap();
        let ev = db.record_play_start(id, 1_786_190_400).unwrap(); // 2026-08-08 12:00 UTC
        db.record_play_end(
            ev,
            &PlayEnd {
                track_id: id,
                ended_at: 1_786_191_600,
                listened_ms: 120_000,
                reason: "ended",
            },
        )
        .unwrap();

        let snap = read_snapshot(&mut db).expect("projection ok");
        assert_eq!(snap.recent.len(), 1, "同曲两次播放收敛为一行");
        assert_eq!(snap.recent_latest, "08-08 12:00");
        assert_eq!(snap.recent_earliest, "08-08 12:00");
        let row = &snap.recent[0];
        assert_eq!(row.mid, key);
        assert_eq!(row.source, 1);
        assert_eq!(row.duration_ms, 200_000, "本地行经曲目表补全时长");
        assert_eq!(row.quality, "FLAC");
    }

    /// 最近播放 QQ 行：播放键随投影下发（source/source_key → mid），
    /// 元数据经库内缓存补全——此前 mid 只能置空，整表播放（PlayList）必败。
    #[test]
    fn recent_rows_qq_row_keeps_playable_mid() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.upsert_track(&hmp_storage::TrackRow {
            source: "qq",
            source_key: "mid-9".into(),
            title: "晴天".into(),
            artist: Some("周杰伦".into()),
            album: Some("叶惠美".into()),
            duration_ms: Some(269_000),
            ..Default::default()
        })
        .unwrap();
        let id = db.track_id("qq", "mid-9").unwrap().unwrap();
        let ev = db.record_play_start(id, 1_786_190_400).unwrap();
        db.record_play_end(
            ev,
            &PlayEnd {
                track_id: id,
                ended_at: 1_786_191_600,
                listened_ms: 120_000,
                reason: "ended",
            },
        )
        .unwrap();

        let snap = read_snapshot(&mut db).expect("projection ok");
        assert_eq!(snap.recent.len(), 1);
        let row = &snap.recent[0];
        assert_eq!(row.mid, "mid-9", "QQ 行 mid 必须可回放");
        assert_eq!(row.source, 0);
        assert_eq!(row.title, "晴天");
        assert_eq!(row.artists, "周杰伦");
        assert_eq!(row.album, "叶惠美");
    }

    /// 文件夹分组：按扫描根前缀聚合（嵌套根取最长优先）；根外曲目不入任何组；
    /// last_scan 无数据源恒 "—"；容量 = 现文件 stat 聚合。
    #[test]
    fn folders_aggregate_by_scan_root_longest_prefix() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let root = test_dir("folders");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let other = test_dir("folders-other");
        let a = root.join("a.mp3");
        let b = sub.join("b.flac");
        let c = other.join("c.mp3");
        std::fs::write(&a, vec![0u8; 1000]).unwrap();
        std::fs::write(&b, vec![0u8; 2000]).unwrap();
        std::fs::write(&c, vec![0u8; 4000]).unwrap();

        let (root_id, generation) = db.begin_scan(&root).unwrap();
        db.record_scan_file(
            root_id,
            generation,
            &a,
            Some(&meta("A", "ar", "al", 1000, "mp3")),
            "fp-a",
        )
        .unwrap();
        // 嵌套根：sub 注册在后代际，b 归内层。
        let (sub_id, sub_generation) = db.begin_scan(&sub).unwrap();
        db.record_scan_file(
            sub_id,
            sub_generation,
            &b,
            Some(&meta("B", "ar", "al", 2000, "flac")),
            "fp-b",
        )
        .unwrap();
        // 根外文件（模拟收藏/单文件入库路径）：不出现在任何文件夹组。
        let (root_id2, generation2) = db.begin_scan(&root).unwrap();
        db.record_scan_file(root_id2, generation2, &c, None, "fp-c")
            .unwrap();

        let snap = read_snapshot(&mut db).expect("projection ok");
        assert_eq!(snap.local_tracks.len(), 3);
        assert_eq!(snap.folders.len(), 2, "两个注册根各成一行");
        assert_eq!(snap.folders[0].path, root.display().to_string());
        assert_eq!(snap.folders[0].track_count, 1);
        assert_eq!(snap.folders[0].size_bytes, 1000);
        assert_eq!(snap.folders[0].last_scan, "—");
        assert_eq!(snap.folders[1].path, sub.display().to_string());
        assert_eq!(snap.folders[1].track_count, 1);
        assert_eq!(snap.folders[1].size_bytes, 2000);
        // 行级归属：b 命中最长前缀 sub；a 命中 root；c 无归属。
        let row_b = snap.local_tracks.iter().find(|r| r.title == "B").unwrap();
        assert_eq!(
            row_b.folder.as_deref(),
            Some(sub.display().to_string()).as_deref()
        );
        assert_eq!(row_b.quality, "FLAC");
        let row_c = snap.local_tracks.iter().find(|r| r.title == "c").unwrap();
        assert!(row_c.folder.is_none());
        // 总容量 = 各组聚合（根外文件不计入）。
        assert_eq!(snap.local_size_text, format_bytes(3000));
    }

    /// 歌单分流：local/owned → 自建组，subscribed → 收藏组；副标题 "N 首"。
    #[test]
    fn playlists_split_by_relation() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let p1 = db.create_playlist("本地自建").unwrap();
        db.add_playlist_track(p1, "qq", "mid-1", "A").unwrap();
        db.add_playlist_track(p1, "qq", "mid-2", "B").unwrap();
        db.reconcile_playlist("dirid-77", "远端自建", "owned")
            .unwrap();
        db.reconcile_playlist("disstid-88", "收藏的歌单", "subscribed")
            .unwrap();

        let snap = read_snapshot(&mut db).expect("projection ok");
        assert_eq!(snap.created.len(), 2);
        assert_eq!(snap.favorited.len(), 1);

        let local = snap.created.iter().find(|p| p.name == "本地自建").unwrap();
        assert_eq!(local.id, p1.to_string());
        assert_eq!(local.subtitle, "2 首");
        assert!(snap.created.iter().any(|p| p.name == "远端自建"));
        assert_eq!(snap.favorited[0].name, "收藏的歌单");
        assert_eq!(snap.favorited[0].subtitle, "0 首");
    }

    /// 歌单详情：position 序；本地行补全元数据，QQ 行经 meta 补全歌手；
    /// 不存在的歌单 → None。
    #[test]
    fn playlist_detail_enriches_rows() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = test_dir("pl-detail");
        let f = dir.join("a.flac");
        db.add_local_file(
            &f,
            Some(&meta("夜曲", "周杰伦", "十一月的萧邦", 180_000, "flac")),
        )
        .unwrap();
        let key = format!("local:{}", f.display());
        db.upsert_track(&hmp_storage::TrackRow {
            source: "qq",
            source_key: "mid-9".into(),
            title: "晴天".into(),
            artist: Some("周杰伦".into()),
            album: Some("叶惠美".into()),
            ..Default::default()
        })
        .unwrap();
        let p = db.create_playlist("晚间循环").unwrap();
        db.add_playlist_track(p, "qq", "mid-9", "晴天").unwrap();
        db.add_playlist_track(p, "local", &key, "夜曲").unwrap();

        let detail = playlist_detail_from(&mut db, p).expect("detail ok");
        assert_eq!(detail.name, "晚间循环");
        assert_eq!(detail.tracks.len(), 2);
        assert_eq!(detail.total_ms, 180_000, "QQ 行 0ms，本地行 180s");
        // position 序：先加的 QQ 行在前。
        assert_eq!(detail.tracks[0].mid, "mid-9");
        assert_eq!(detail.tracks[0].artists, "周杰伦");
        assert_eq!(detail.tracks[1].mid, key);
        assert_eq!(detail.tracks[1].quality, "FLAC");

        assert!(playlist_detail_from(&mut db, p + 100).is_none());
    }

    /// 专辑详情：按名精确（NOCASE）命中组行与曲目；子串名不误命中。
    #[test]
    fn album_detail_matches_exact_nocase() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = test_dir("album-detail");
        let a = dir.join("a.flac");
        let b = dir.join("b.mp3");
        db.add_local_file(
            &a,
            Some(&meta("夜曲", "周杰伦", "十一月的萧邦", 180_000, "flac")),
        )
        .unwrap();
        db.add_local_file(
            &b,
            Some(&meta("发如雪", "周杰伦", "十一月的萧邦", 200_000, "mp3")),
        )
        .unwrap();
        db.set_track_cover(
            &format!("local:{}", a.display()),
            "file:///covers/november.jpg",
        )
        .unwrap();

        let detail = album_detail_from(&mut db, "十一月的萧邦").expect("detail ok");
        assert_eq!(detail.name, "十一月的萧邦");
        assert_eq!(detail.artist.as_deref(), Some("周杰伦"));
        assert_eq!(detail.tracks.len(), 2, "同专辑两行");
        assert_eq!(detail.total_ms, 380_000);
        assert_eq!(
            detail.tracks[0].cover_uri.as_deref(),
            Some("file:///covers/november.jpg")
        );
        // 大小写不敏感；子串不算命中。
        assert!(album_detail_from(&mut db, "十一月的萧邦".to_uppercase().as_str()).is_some());
        assert!(album_detail_from(&mut db, "萧邦").is_none());
        assert!(album_detail_from(&mut db, "不存在的专辑").is_none());
    }

    /// 歌手详情：曲目按 track_artists 命中；展示串含分隔符时按最长包含归一；
    /// 专辑 = 曲目出现过的专辑（年份降序、缺年份垫底）。
    #[test]
    fn artist_detail_resolves_canonical_name_and_albums() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = test_dir("artist-detail");
        let a = dir.join("a.flac");
        let b = dir.join("b.flac");
        // add_local_file 将 meta.artist 写入 track_artists（单值口径）。
        db.add_local_file(&a, Some(&meta("歌一", "群星", "合辑一", 100_000, "flac")))
            .unwrap();
        db.add_local_file(&b, Some(&meta("歌二", "群星", "合辑二", 120_000, "flac")))
            .unwrap();

        // "群星" 精确命中两首；专辑两行（无年份 → 垫底按名序）。
        let detail = artist_detail_from(&mut db, "群星").expect("detail ok");
        assert_eq!(detail.name, "群星");
        assert_eq!(detail.tracks.len(), 2);
        assert_eq!(detail.albums.len(), 2);
        assert_eq!(detail.albums[0].year_text, "—");

        // 含分隔符的展示串：库内名是其子串 → 归一到 "群星"。
        let resolved = artist_detail_from(&mut db, "群星 / 某合唱团").expect("resolved");
        assert_eq!(resolved.name, "群星");
        assert_eq!(resolved.tracks.len(), 2);

        // 完全无关的名字 → 空详情（found 但无内容）；空参 → None。
        let empty = artist_detail_from(&mut db, "不存在的歌手").expect("detail ok");
        assert!(empty.tracks.is_empty() && empty.albums.is_empty());
        assert!(artist_detail_from(&mut db, "").is_none());
    }
}
