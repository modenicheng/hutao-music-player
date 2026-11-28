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
//!
//! 导航身份（§30.2.6 phase 2b）：行上 `artist_id`/`album_id`（本地实体）与
//! `artist_mid`/`album_mid`（QQ）由 `SongRow::to_track_row` 生成
//! `artist:<id>` / `album:<id>` / `qq:<mid>` 参数；实体缺失回退裸展示名
//! （旧按名路径）。存储读投影 `LibraryTrackRow` 目前不带行级实体 id/mid：
//! 本地行经 `library_artists`/`library_albums` 的名称索引反查实体，
//! QQ 行经 `track_artists`/`album_mid` 逐行取 mid。同名专辑的另一实体、
//! 多歌手展示串里的非首位歌手在索引层无法唯一对应时按契约回退展示名——
//! 待存储读接口补行级列后换成直接投影（报告里的接口缺口）。

use std::collections::HashMap;
use std::path::Path;

use hmp_storage::db::{AlbumGroup, LibraryTrackRow};
use hmp_storage::{LibraryDb, TrackMeta};
use slint::Image;

use crate::TrackRow;
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
    /// 歌手实体 id（本地行；0 = 无实体/QQ 行）。导航参数 `artist:<id>`。
    pub artist_id: i64,
    /// 专辑实体 id（本地行；0 = 无实体/QQ 行）。导航参数 `album:<id>`。
    pub album_id: i64,
    /// QQ 歌手 mid（QQ 行取 `track_artists.artist_mid`；本地行取实体登记的
    /// QQ mid）。导航参数 `qq:<mid>`。
    pub artist_mid: Option<String>,
    /// QQ 专辑 mid（QQ 行取 `tracks.album_mid`；本地行取实体登记的 QQ mid）。
    pub album_mid: Option<String>,
    pub duration_ms: i32,
    /// 音质徽章文案；无损以下为空（对齐 mock 纪律）。
    pub quality: String,
    /// 本地封面 URI（`file://…`，扫描提取/rebind 回写产物）；缺失 → 渲染时
    /// 走程序化封面。QQ http 封面不落此列（项目原则禁 HTTP）。
    pub cover_uri: Option<String>,
    /// QQ 远程封面 URL（库内 tracks.cover_uri 的 http 形态，预取输入）：
    /// UI 禁直连 HTTP，交列表预取器经 daemon CoverGet 换本地产物后原地补图。
    pub remote_cover: Option<String>,
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
    /// QQ 歌单封面本地产物（reconcile 取得后回写；None → 程序化占位）。
    pub cover_uri: Option<String>,
    /// 歌单归属：`local`=纯本地，`owned`=QQ 自建（右键"加入播放列表"
    /// 按曲源兼容性预禁用用）。
    pub relation: String,
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
    let entities = LibraryEntities::build(db)?;
    let roots = db.scan_roots().ok()?;
    let local_rows_vec = db.library_tracks(None, None, None, false).ok()?;
    let local_by_key = index_by_key(&local_rows_vec);

    let liked = liked_rows(db, &entities, &local_by_key)?;
    let (recent, recent_latest, recent_earliest) = recent_rows(db, &entities, &local_by_key)?;
    let (local_tracks, folders) = local_rows(db, &local_rows_vec, &entities, &roots)?;
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

/// 全量本地曲目表按源键索引（我喜欢 / 最近播放的本地行补全数据源）。
fn index_by_key(rows: &[LibraryTrackRow]) -> HashMap<String, LibraryTrackRow> {
    rows.iter()
        .map(|t| (t.source_key.clone(), t.clone()))
        .collect()
}

/// 「最近播放」页单独重读（D7）：引擎写播放历史不发 `LibraryChanged`，库页
/// 刷新到不了这一页——进页导航时由桥在后台调用。库缺失/打开/投影失败 →
/// None（调用方保持现有模型不动）。
pub fn load_recent() -> Option<(Vec<SongRow>, String, String)> {
    with_db(read_recent)
}

/// 最近播放页投影：与快照同一口径（`recent_rows`），本地行补全所需的
/// 实体索引与曲目表在此单独取一次。
fn read_recent(db: &mut LibraryDb) -> Option<(Vec<SongRow>, String, String)> {
    let entities = LibraryEntities::build(db)?;
    let local_by_key = index_by_key(&db.library_tracks(None, None, None, false).ok()?);
    recent_rows(db, &entities, &local_by_key)
}

// ————————————————————————————————————————————————————————————
// 各页投影
// ————————————————————————————————————————————————————————————

/// 专辑实体索引（`library_albums` 单次查询的投影）：封面 / 行级实体反查 /
/// 同名歧义判定。同名不同专辑歌手在库里是两个实体；行上只有展示串，
/// 因此只在 (标题, 展示歌手) 唯一或标题唯一时反查，否则诚实回退裸名。
#[derive(Clone, Debug, Default)]
struct AlbumIndex {
    /// `library_albums` 原序组行（同名按 SQL 名序；详情按 id 取）。
    groups: Vec<AlbumGroup>,
    /// 标题 → 封面 URI（专辑级封面；扫描为整专辑提取同源内嵌图）。
    covers: HashMap<String, String>,
    /// (标题小写, 专辑歌手小写) → 实体 id（0 号历史直插行不入索引）。
    ids_by_key: HashMap<(String, String), i64>,
    /// 标题小写 → 该标题下的实体 id（同名歧义判定用）。
    ids_by_title: HashMap<String, Vec<i64>>,
    /// 实体 id → `groups` 下标。
    by_id: HashMap<i64, usize>,
}

impl AlbumIndex {
    fn build(db: &mut LibraryDb) -> Option<Self> {
        let groups = db.library_albums(None).ok()?;
        let mut index = Self::default();
        for (i, group) in groups.iter().enumerate() {
            if let Some(uri) = group.cover_uri.as_ref() {
                // 同名专辑沿用旧 `album_cover_map` 的 collect 口径（后写覆盖）。
                index.covers.insert(group.album.clone(), uri.clone());
            }
            if group.id <= 0 {
                continue;
            }
            index.by_id.insert(group.id, i);
            let title = group.album.trim().to_lowercase();
            index
                .ids_by_title
                .entry(title.clone())
                .or_default()
                .push(group.id);
            let artist = group
                .artist
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            index.ids_by_key.insert((title, artist), group.id);
        }
        index.groups = groups;
        Some(index)
    }

    fn group_by_id(&self, id: i64) -> Option<&AlbumGroup> {
        self.by_id.get(&id).and_then(|i| self.groups.get(*i))
    }

    /// 本地行 → 专辑实体：先 (标题, 展示歌手) 精确命中，再同名唯一兜底；
    /// 同名多实体且展示串对不上 → None（调用方回退裸名旧路径）。
    fn for_row(&self, album: &str, artist: &str) -> Option<&AlbumGroup> {
        let title = album.trim().to_lowercase();
        let artist = artist.trim().to_lowercase();
        if let Some(id) = self.ids_by_key.get(&(title.clone(), artist)) {
            return self.group_by_id(*id);
        }
        match self.ids_by_title.get(&title)?.as_slice() {
            [id] => self.group_by_id(*id),
            _ => None,
        }
    }

    fn cover(&self, album: &str) -> Option<String> {
        self.covers.get(album).cloned()
    }
}

/// 库内实体索引（本地行导航 id 反查）：歌手名与专辑组行。
#[derive(Clone, Debug, Default)]
struct LibraryEntities {
    /// 歌手名小写 → (实体 id, 实体登记的 QQ mid)。
    artists: HashMap<String, (i64, Option<String>)>,
    albums: AlbumIndex,
}

impl LibraryEntities {
    fn build(db: &mut LibraryDb) -> Option<Self> {
        let mut artists = HashMap::new();
        for group in db.library_artists().ok()? {
            if group.id > 0 {
                artists
                    .entry(group.artist.trim().to_lowercase())
                    .or_insert((group.id, group.artist_mid));
            }
        }
        Some(Self {
            artists,
            albums: AlbumIndex::build(db)?,
        })
    }

    /// 本地行展示歌手 → 歌手实体：整串优先（单歌手标签），否则取 `" / "`
    /// 连接的第一位（与写入侧 `join_artist_names` 同分隔符）；多歌手行
    /// 的链接统一指向主歌手（PORTING.md 决策 5）。
    fn artist_for(&self, display: &str) -> Option<(i64, Option<String>)> {
        let display = display.trim();
        if display.is_empty() {
            return None;
        }
        if let Some(entity) = self.artists.get(&display.to_lowercase()) {
            return Some(entity.clone());
        }
        let first = display.split(" / ").next()?.trim();
        self.artists.get(&first.to_lowercase()).cloned()
    }
}

/// 行级实体导航投影（本地实体 id 与 QQ mid 同一载体，构造 SongRow 用）。
#[derive(Clone, Debug, Default)]
struct SongEntities {
    artist_id: i64,
    album_id: i64,
    artist_mid: Option<String>,
    album_mid: Option<String>,
}

/// QQ 行 → (歌手 mid, 专辑 mid)：`track_artists` / `tracks.album_mid` 是
/// 存储层仅有的行级 mid 读口（`TrackMeta` 不含 mid）。页面行数有限
/// （收藏/最近播放/歌单详情），逐行两次索引查询。
fn qq_entity_map(
    db: &mut LibraryDb,
    track_ids: impl IntoIterator<Item = i64>,
) -> HashMap<i64, SongEntities> {
    track_ids
        .into_iter()
        .map(|track_id| {
            let artist_mid = db.track_artists(track_id).ok().and_then(|refs| {
                refs.into_iter()
                    .find_map(|artist| artist.mid.filter(|mid| !mid.trim().is_empty()))
            });
            let album_mid = db
                .album_mid(track_id)
                .ok()
                .flatten()
                .filter(|mid| !mid.trim().is_empty());
            (
                track_id,
                SongEntities {
                    artist_mid,
                    album_mid,
                    ..SongEntities::default()
                },
            )
        })
        .collect()
}

/// 我喜欢：`list_favorites`（本地事实视图，新→旧）为权威序；
/// 本地行经全量本地曲目表补全元数据，QQ 行经 `track_meta_batch` 补全歌手/专辑。
fn liked_rows(
    db: &mut LibraryDb,
    entities: &LibraryEntities,
    local_by_key: &HashMap<String, LibraryTrackRow>,
) -> Option<Vec<SongRow>> {
    let favs = db.list_favorites(LIKED_LIMIT).ok()?;
    let qq_meta = qq_meta_map(db, favs.iter().map(|f| f.source_key.clone()).collect());
    let qq_entities = qq_entity_map(
        db,
        favs.iter()
            .filter(|f| f.source != "local")
            .map(|f| f.track_id),
    );

    Some(
        favs.into_iter()
            .map(|f| match f.source.as_str() {
                "local" => match local_by_key.get(f.source_key.as_str()) {
                    // 与详情投影共用口径（时长/音质/封面按专辑聚合）
                    Some(t) => song_row_from_local(db, t, entities),
                    // 收藏先于扫描入库（无 local_files 行）：稀疏行诚实展示。
                    None => SongRow {
                        mid: f.source_key.clone(),
                        source: 1,
                        title: f.title,
                        artists: String::new(),
                        album: String::new(),
                        artist_id: 0,
                        album_id: 0,
                        artist_mid: None,
                        album_mid: None,
                        duration_ms: 0,
                        quality: quality_text(format_of_key(&f.source_key)),
                        cover_uri: None,
                        remote_cover: None,
                        folder: None,
                    },
                },
                // QQ 行：时长/歌手/专辑/封面走 track_meta_batch 库内缓存
                // （远程封面 URL 交预取器，见 song_row_from_qq）。
                _ => song_row_from_qq(
                    &f.source_key,
                    f.title,
                    qq_meta.get(f.source_key.as_str()),
                    qq_entities.get(&f.track_id).cloned().unwrap_or_default(),
                ),
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
    entities: &LibraryEntities,
    local_by_key: &HashMap<String, LibraryTrackRow>,
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
    let qq_entities = qq_entity_map(
        db,
        plays
            .iter()
            .filter(|p| p.source != "local")
            .map(|p| p.track_id),
    );
    let rows = plays
        .into_iter()
        .map(|play| match play.source.as_str() {
            "local" => {
                let key = play.source_key;
                match local_by_key.get(key.as_str()) {
                    Some(t) => {
                        // 展示名沿用播放记录（历史行可能带旧标签），实体 id /
                        // 封面 / 时长仍按曲目行投影（与详情页同口径）。
                        let mut row = song_row_from_local(db, t, entities);
                        row.mid = key;
                        row.title = play.title;
                        row.artists = play.artist.unwrap_or_default();
                        row
                    }
                    None => {
                        let quality = quality_text(format_of_key(&key));
                        SongRow {
                            mid: key,
                            source: 1,
                            title: play.title,
                            artists: play.artist.unwrap_or_default(),
                            album: String::new(),
                            artist_id: 0,
                            album_id: 0,
                            artist_mid: None,
                            album_mid: None,
                            duration_ms: 0,
                            quality,
                            cover_uri: None,
                            remote_cover: None,
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
                qq_entities.get(&play.track_id).cloned().unwrap_or_default(),
            ),
        })
        .collect();
    Some((rows, latest, earliest))
}

/// 音乐库：全部本地曲目行（library_tracks ORDER BY title）+ 按扫描根前缀
/// 分组的文件夹统计（嵌套根取最长前缀，与 `scan_root_for` 同语义）。
fn local_rows(
    db: &mut LibraryDb,
    rows: &[LibraryTrackRow],
    entities: &LibraryEntities,
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
            let mut row = song_row_from_local(db, t, entities);
            row.folder = root_idx.map(|i| roots[i].clone());
            row
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
            cover_uri: p.cover_uri,
            relation: p.relation.clone(),
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
    /// 歌单归属：`local` / `owned` / `subscribed`（歌单页级菜单按它裁剪
    /// 重命名/删除语义；daemon 对 owned/subscribed 的重命名报错）
    pub relation: String,
    pub tracks: Vec<SongRow>,
    pub total_ms: u64,
    /// 封面本地产物 URI（QQ 歌单 reconcile 回写；None → 程序化占位）。
    pub cover_uri: Option<String>,
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
    /// 专辑卡导航参数：`album:<id>`；历史行（无实体）回退专辑名旧路径。
    pub param: String,
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
    let (name, cover_uri, relation) = db
        .list_playlists()
        .ok()?
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| (p.name, p.cover_uri, p.relation))?;
    let rows = db.playlist_tracks(id).ok()?;
    let local_by_key = local_track_map(db)?;
    let entities = LibraryEntities::build(db)?;
    let qq_meta = qq_meta_map(db, rows.iter().map(|r| r.source_key.clone()).collect());
    let qq_entities = qq_entity_map(
        db,
        rows.iter()
            .filter(|r| !r.source_key.starts_with("local:"))
            .map(|r| r.track_id),
    );

    let tracks: Vec<SongRow> = rows
        .iter()
        .map(|row| match row.source_key.strip_prefix("local:") {
            Some(_) => match local_by_key.get(row.source_key.as_str()) {
                Some(t) => song_row_from_local(db, t, &entities),
                // 本地曲目已被移出扫描（收藏/歌单行仍在）：稀疏行诚实展示。
                None => SongRow {
                    mid: row.source_key.clone(),
                    source: 1,
                    title: row.title.clone(),
                    artists: String::new(),
                    album: String::new(),
                    artist_id: 0,
                    album_id: 0,
                    artist_mid: None,
                    album_mid: None,
                    duration_ms: 0,
                    quality: quality_text(format_of_key(&row.source_key)),
                    cover_uri: None,
                    remote_cover: None,
                    folder: None,
                },
            },
            None => song_row_from_qq(
                &row.source_key,
                row.title.clone(),
                qq_meta.get(&row.source_key),
                qq_entities.get(&row.track_id).cloned().unwrap_or_default(),
            ),
        })
        .collect();
    let total_ms: u64 = tracks.iter().map(|r| r.duration_ms.max(0) as u64).sum();
    Some(PlaylistDetail {
        name,
        relation,
        cover_uri,
        tracks,
        total_ms,
    })
}

/// 本地实体导航参数前缀（`Nav.param` 契约，§30.2.6 phase 2b）。
pub const ARTIST_PARAM_PREFIX: &str = "artist:";
/// 专辑实体导航参数前缀（同上）。
pub const ALBUM_PARAM_PREFIX: &str = "album:";

/// `artist:<id>` / `album:<id>` → 实体 id；前缀不符或 id 非正整数 → None
/// （走旧按名路径——展示名不能被误判成实体参数）。
fn entity_id_param(param: &str, prefix: &str) -> Option<i64> {
    param
        .strip_prefix(prefix)?
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
}

/// 专辑详情：`album:<id>` 按实体 id（同名另一实体不混入）；否则按专辑名
/// （大小写不敏感精确）走旧路径。头部元数据（主歌手/年份/封面）取组行。
pub fn album_detail(param: &str) -> Option<AlbumDetail> {
    with_db(|db| album_detail_from(db, param))
}

fn album_detail_from(db: &mut LibraryDb, param: &str) -> Option<AlbumDetail> {
    if let Some(album_id) = entity_id_param(param, ALBUM_PARAM_PREFIX) {
        return album_detail_by_id(db, album_id);
    }
    let entities = LibraryEntities::build(db)?;
    // 组行按专辑名精确（NOCASE）命中，避免"Love Story"命中"Love Stories"。
    let group = entities
        .albums
        .groups
        .iter()
        .find(|a| a.album.eq_ignore_ascii_case(param))?
        .clone();
    let track_rows = db.local_tracks_by_album(param).ok()?;
    let mut tracks: Vec<SongRow> = track_rows
        .iter()
        .map(|t| song_row_from_local(db, t, &entities))
        .collect();
    // 旧口径：同名专辑的行共用组行封面（行级封面列未暴露）。
    if let Some(uri) = group.cover_uri.as_ref() {
        for row in &mut tracks {
            row.cover_uri = Some(uri.clone());
        }
    }
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

/// 按实体 id 取专辑详情（`album_tracks_by_id` 只回该实体曲目）。
/// 实体存在但本地无曲目（QQ-only 实体）→ None（诚实 miss，不伪造空页）。
fn album_detail_by_id(db: &mut LibraryDb, album_id: i64) -> Option<AlbumDetail> {
    let entity = db.album_by_id(album_id).ok()??;
    let track_rows = db.album_tracks_by_id(album_id).ok()?;
    if track_rows.is_empty() {
        return None;
    }
    let entities = LibraryEntities::build(db)?;
    let cover_uri = entities
        .albums
        .group_by_id(album_id)
        .and_then(|group| group.cover_uri.clone());
    let mut tracks: Vec<SongRow> = track_rows
        .iter()
        .map(|t| song_row_from_local(db, t, &entities))
        .collect();
    if let Some(uri) = cover_uri.as_ref() {
        for row in &mut tracks {
            row.cover_uri = Some(uri.clone());
        }
    }
    let total_ms: u64 = tracks.iter().map(|r| r.duration_ms.max(0) as u64).sum();
    Some(AlbumDetail {
        name: entity.title,
        artist: entity.album_artist,
        year: entity.year,
        cover_uri,
        tracks,
        total_ms,
    })
}

/// 歌手详情：`artist:<id>` 按实体 id（同名实体不混入，也不走"最长包含
/// 匹配"）；否则按旧展示名路径（曲目行展示串可能含分隔符，归一到库内名）。
/// 专辑 = 该歌手曲目出现过的专辑（组行提供年份/封面/全碟曲目数）。
pub fn artist_detail(param: &str) -> Option<ArtistDetail> {
    with_db(|db| artist_detail_from(db, param))
}

fn artist_detail_from(db: &mut LibraryDb, param: &str) -> Option<ArtistDetail> {
    if let Some(artist_id) = entity_id_param(param, ARTIST_PARAM_PREFIX) {
        let entity = db.artist_by_id(artist_id).ok()??;
        let tracks = db.artist_tracks_by_id(artist_id).ok()?;
        if tracks.is_empty() {
            return None;
        }
        return artist_detail_from_rows(db, entity.name, tracks);
    }
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
    // 本地投影 miss 判定（D2c）：曲目空 = 该歌手不在本地库（含 qq: 远端
    // 参数——精确 artist 匹配必空）。空曲目页无信息量且会拦掉远端分流，
    // 退回 None 走「未找到」/远端装载（专辑页同口径：分组无命中即 None）。
    if tracks.is_empty() {
        return None;
    }
    artist_detail_from_rows(db, name, tracks)
}

/// 歌手详情的共用投影（按名/按 id 两条路径同口径）。
fn artist_detail_from_rows(
    db: &mut LibraryDb,
    name: String,
    tracks: Vec<LibraryTrackRow>,
) -> Option<ArtistDetail> {
    let entities = LibraryEntities::build(db)?;
    let mut albums: Vec<ArtistAlbum> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in &tracks {
        let Some(album) = t.album.as_deref() else {
            continue;
        };
        if !seen.insert(album.to_string()) {
            continue;
        }
        let Some(group) = entities
            .albums
            .for_row(album, t.artist.as_deref().unwrap_or_default())
        else {
            continue;
        };
        albums.push(ArtistAlbum {
            name: group.album.clone(),
            param: album_nav_param(group),
            year_text: group
                .year
                .map(|y| y.to_string())
                .unwrap_or_else(|| "—".into()),
            track_count: group.track_count as i32,
            cover_uri: group.cover_uri.clone(),
        });
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
    let track_rows: Vec<SongRow> = tracks
        .iter()
        .map(|t| song_row_from_local(db, t, &entities))
        .collect();
    Some(ArtistDetail {
        name,
        tracks: track_rows,
        albums,
    })
}

/// 歌手页专辑卡导航参数：实体 id 优先，历史行（id=0）回退专辑名旧路径。
fn album_nav_param(group: &AlbumGroup) -> String {
    if group.id > 0 {
        format!("{ALBUM_PARAM_PREFIX}{}", group.id)
    } else {
        group.album.clone()
    }
}

/// QQ mid → `qq:<mid>` 导航参数；空串/全空白/无 mid → None（回退展示名）。
fn qq_mid_param(mid: Option<&str>) -> Option<String> {
    let mid = mid.map(str::trim).filter(|mid| !mid.is_empty())?;
    Some(format!("{}{mid}", crate::remote_details::REMOTE_PREFIX))
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

/// 本地行 → 专辑实体：名称索引唯一命中直接用；同名多实体且行展示歌手
/// 对不上时，按实体曲目成员关系精确定位（行级 `tracks.album_id` 没有
/// 读口；只在这类歧义行按候选实体查询 `album_tracks_by_id`）。
fn local_album_entity(
    db: &mut LibraryDb,
    entities: &LibraryEntities,
    t: &LibraryTrackRow,
) -> Option<(i64, Option<String>)> {
    let album = t.album.as_deref()?;
    if let Some(group) = entities
        .albums
        .for_row(album, t.artist.as_deref().unwrap_or_default())
    {
        return Some((group.id, group.album_mid.clone()));
    }
    let ids = entities
        .albums
        .ids_by_title
        .get(&album.trim().to_lowercase())?;
    for id in ids {
        let rows = db.album_tracks_by_id(*id).ok()?;
        if rows.iter().any(|row| row.track_id == t.track_id) {
            let group = entities.albums.group_by_id(*id)?;
            return Some((group.id, group.album_mid.clone()));
        }
    }
    None
}

/// 本地行 → SongRow（详情投影与收藏页共用口径：时长/音质来自本地曲目表，
/// 封面按专辑聚合命中；实体 id 经名称索引反查，见 `LibraryEntities`）。
fn song_row_from_local(
    db: &mut LibraryDb,
    t: &LibraryTrackRow,
    entities: &LibraryEntities,
) -> SongRow {
    let artist = t
        .artist
        .as_deref()
        .and_then(|display| entities.artist_for(display));
    let album = local_album_entity(db, entities, t);
    SongRow {
        mid: t.source_key.clone(),
        source: 1,
        title: t.title.clone(),
        artists: t.artist.clone().unwrap_or_default(),
        album: t.album.clone().unwrap_or_default(),
        artist_id: artist.as_ref().map(|(id, _)| *id).unwrap_or(0),
        album_id: album.as_ref().map(|(id, _)| *id).unwrap_or(0),
        artist_mid: artist.and_then(|(_, mid)| mid),
        album_mid: album.and_then(|(_, mid)| mid),
        duration_ms: t.duration_ms.unwrap_or(0) as i32,
        quality: quality_text(format_of_key(&t.source_key)),
        cover_uri: t
            .album
            .as_deref()
            .and_then(|title| entities.albums.cover(title)),
        remote_cover: None,
        folder: None,
    }
}

/// QQ 行 → SongRow：时长/歌手/专辑/封面走 track_meta_batch 库内缓存（reconcile
/// 曲目缓存与播放路径写入）。实体导航只有 mid（QQ 行没有本地实体 id；
/// `entities.artist_mid`/`album_mid` 来自行级 `track_artists`/`tracks.album_mid`）。
/// 封面分流：`file://`（rebind 回写产物）直接读盘；远程 URL 是预取输入
/// （UI 禁直连 HTTP），交列表预取器经 daemon CoverGet 换图；全缺 → 程序化占位。
/// 此前 cover_uri 无条件置空——库里明明有 rebind 产物，列表页也永远显示占位
/// （2026-10-03 封面审计三断点之一）。
fn song_row_from_qq(
    source_key: &str,
    title: String,
    meta: Option<&TrackMeta>,
    entities: SongEntities,
) -> SongRow {
    let (cover_uri, remote_cover) = match meta.and_then(|m| m.cover_uri.clone()) {
        Some(uri) if uri.starts_with("http") => (None, Some(uri)),
        Some(uri) => (Some(uri), None),
        None => (None, None),
    };
    SongRow {
        mid: source_key.to_string(),
        source: 0,
        title,
        artists: meta.and_then(|m| m.artist.clone()).unwrap_or_default(),
        album: meta.and_then(|m| m.album.clone()).unwrap_or_default(),
        artist_id: entities.artist_id,
        album_id: entities.album_id,
        artist_mid: entities.artist_mid,
        album_mid: entities.album_mid,
        // 时长走库内缓存（track_meta_batch 读 tracks.duration_ms；reconcile
        // 曲目缓存/播放路径写入）——此前恒 0，详情页"总时长"永远 0:00。
        duration_ms: meta.and_then(|m| m.duration_ms).unwrap_or(0) as i32,
        quality: String::new(),
        cover_uri,
        remote_cover,
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
    /// 行内歌手/专辑链接的导航参数（`Nav.param` 单串命名空间，§30.2.6 phase 2b）：
    /// - 本地行：实体 id 优先 → `artist:<id>` / `album:<id>`（按 id 查，同名
    ///   另一实体不混入）；id=0（历史直插行）→ 裸展示名（旧按名路径）。
    /// - QQ 行：真 mid → `qq:<mid>`（远端详情）；mid 缺失 → 裸展示名。
    /// - 展示名也为空 → 空串（UI 禁用链接）。
    fn artist_nav_param(&self) -> String {
        if self.source == 1 && self.artist_id > 0 {
            return format!("{ARTIST_PARAM_PREFIX}{}", self.artist_id);
        }
        if self.source != 1
            && let Some(param) = qq_mid_param(self.artist_mid.as_deref())
        {
            return param;
        }
        self.artists.clone()
    }

    /// 专辑导航参数（规则同 [`SongRow::artist_nav_param`]）。
    fn album_nav_param(&self) -> String {
        if self.source == 1 && self.album_id > 0 {
            return format!("{ALBUM_PARAM_PREFIX}{}", self.album_id);
        }
        if self.source != 1
            && let Some(param) = qq_mid_param(self.album_mid.as_deref())
        {
            return param;
        }
        self.album.clone()
    }

    /// SongRow → TrackRow。表格行不带封面（TrackRow 已剥 `cover` 字段）：
    /// 12+ 个表格模型每行背全尺寸位图是 RSS 2.4GB 的根因之一，表格只承担
    /// 文本列；侧栏/详情头图/卡片的封面由行外消费点各自取图，队列行走
    /// QueueRow（player_bridge 队列投影）。
    pub fn to_track_row(&self) -> TrackRow {
        TrackRow {
            mid: self.mid.clone().into(),
            source: self.source,
            title: self.title.clone().into(),
            artists: self.artists.clone().into(),
            artist_mid: self.artist_nav_param().into(),
            album: self.album.clone().into(),
            album_mid: self.album_nav_param().into(),
            duration_ms: self.duration_ms,
            quality: self.quality.clone().into(),
        }
    }
}

/// 本地封面读盘（`file://` URI）→ cover_cache 256 桶（对外签名不变，契约第 5
/// 条；bridge.rs 侧栏/详情/卡片调用点自动受益于降采样）。同一专辑封面被多条
/// 曲目复用：file_uri_to_path 后同路径同 key，cover_cache（(路径, 桶) 加权
/// LRU）天然去重。QQ http 封面：项目原则禁 HTTP，不绕。
pub fn local_cover_image(uri: Option<&str>) -> Option<Image> {
    let uri = uri?;
    if !uri.starts_with("file://") {
        return None;
    }
    let path = crate::covers::file_uri_to_path(uri)?;
    crate::cover_cache::get_or_load(&path, 256)
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
        assert_eq!(
            qq.duration_ms, 269_000,
            "QQ 行时长走 track_meta_batch 库内缓存"
        );
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

    /// D7：进页重读与快照同一投影——首次读取之后新产生的播放必须进入最近
    /// 播放（写播放历史不发 LibraryChanged，此前只在库变更时才重查）。
    #[test]
    fn recent_reload_reflects_plays_after_first_read() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = test_dir("recent-reload");
        let play = |db: &mut LibraryDb, file: &str, title: &str, started_at: i64| {
            let f = dir.join(file);
            db.add_local_file(&f, Some(&meta(title, "歌手", "专辑", 200_000, "flac")))
                .unwrap();
            let key = format!("local:{}", f.display());
            let id = db.track_id("local", &key).unwrap().unwrap();
            let ev = db.record_play_start(id, started_at).unwrap();
            db.record_play_end(
                ev,
                &PlayEnd {
                    track_id: id,
                    ended_at: started_at + 600,
                    listened_ms: 90_000,
                    reason: "ended",
                },
            )
            .unwrap();
        };
        play(&mut db, "old.flac", "旧歌", 1_785_000_000); // 2026-07-25 17:20 UTC

        let (rows, latest, earliest) = read_recent(&mut db).expect("projection ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "旧歌");
        assert_eq!(
            (latest.as_str(), earliest.as_str()),
            ("07-25 17:20", "07-25 17:20")
        );

        // 之后的播放：没有任何库变更事件，重读即见（新播置顶）
        play(&mut db, "later.flac", "新歌", 1_786_190_400); // 2026-08-08 12:00 UTC
        let (rows, latest, earliest) = read_recent(&mut db).expect("projection ok");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "新歌", "再播置顶");
        assert_eq!(rows[1].title, "旧歌");
        assert_eq!(latest, "08-08 12:00");
        assert_eq!(earliest, "07-25 17:20");
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
        // begin_scan 存 canonical 路径；期望值同口径（Windows TEMP 环境变量
        // 拼写与盘上真实大小写可能不同）。
        assert_eq!(
            snap.folders[0].path,
            hmp_storage::canonical_display_path(&root)
                .display()
                .to_string()
        );
        assert_eq!(snap.folders[0].track_count, 1);
        assert_eq!(snap.folders[0].size_bytes, 1000);
        assert_eq!(snap.folders[0].last_scan, "—");
        assert_eq!(
            snap.folders[1].path,
            hmp_storage::canonical_display_path(&sub)
                .display()
                .to_string()
        );
        assert_eq!(snap.folders[1].track_count, 1);
        assert_eq!(snap.folders[1].size_bytes, 2000);
        // 行级归属：b 命中最长前缀 sub；a 命中 root；c 无归属。
        let row_b = snap.local_tracks.iter().find(|r| r.title == "B").unwrap();
        assert_eq!(
            row_b.folder.as_deref(),
            Some(
                hmp_storage::canonical_display_path(&sub)
                    .display()
                    .to_string()
            )
            .as_deref()
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
            duration_ms: Some(200_000),
            ..Default::default()
        })
        .unwrap();
        let p = db.create_playlist("晚间循环").unwrap();
        db.add_playlist_track(p, "qq", "mid-9", "晴天").unwrap();
        db.add_playlist_track(p, "local", &key, "夜曲").unwrap();

        let detail = playlist_detail_from(&mut db, p).expect("detail ok");
        assert_eq!(detail.name, "晚间循环");
        assert_eq!(detail.tracks.len(), 2);
        assert_eq!(
            detail.total_ms, 380_000,
            "QQ 行 200s（库内缓存时长）+ 本地行 180s"
        );
        // position 序：先加的 QQ 行在前。
        assert_eq!(detail.tracks[0].mid, "mid-9");
        assert_eq!(detail.tracks[0].artists, "周杰伦");
        assert_eq!(detail.tracks[0].duration_ms, 200_000, "QQ 行时长走缓存");
        assert_eq!(detail.tracks[1].mid, key);
        assert_eq!(detail.tracks[1].quality, "FLAC");

        assert!(playlist_detail_from(&mut db, p + 100).is_none());
    }

    /// QQ 行时长投影：track_meta_batch 的 duration_ms 流入 SongRow
    /// （回归：此前恒 0，歌单详情页"总时长"永远 0:00）。
    #[test]
    fn qq_row_carries_cached_duration() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.upsert_track(&hmp_storage::TrackRow {
            source: "qq",
            source_key: "mid-dur".into(),
            title: "有时长".into(),
            duration_ms: Some(95_000),
            ..Default::default()
        })
        .unwrap();
        let meta = db.track_meta_batch("qq", &["mid-dur".to_owned()]).unwrap();
        let row = song_row_from_qq(
            "mid-dur",
            "有时长".into(),
            Some(&meta[0]),
            SongEntities::default(),
        );
        assert_eq!(row.duration_ms, 95_000);
        // 无缓存行/无时长列 → 诚实 0。
        let row = song_row_from_qq("mid-none", "无时长".into(), None, SongEntities::default());
        assert_eq!(row.duration_ms, 0);
    }

    /// QQ 行封面投影（2026-10-03 封面审计回归：此前无条件置空——库里已有
    /// rebind 产物列表页也显示占位）：file:// → cover_uri 直读盘；http →
    /// remote_cover 预取输入；两者按 cover_uri 形态分流。
    #[test]
    fn qq_row_splits_cover_into_local_and_remote() {
        let meta = |cover: Option<&str>| hmp_storage::TrackMeta {
            source: "qq".into(),
            source_key: "mid".into(),
            title: "曲".into(),
            artist: None,
            album: None,
            duration_ms: None,
            cover_uri: cover.map(String::from),
        };
        let rebound = song_row_from_qq(
            "mid",
            "曲".into(),
            Some(&meta(Some("file://C:/covers/aa.jpg"))),
            SongEntities::default(),
        );
        assert_eq!(
            rebound.cover_uri.as_deref(),
            Some("file://C:/covers/aa.jpg")
        );
        assert_eq!(rebound.remote_cover, None, "本地产物不再预取");

        let remote = song_row_from_qq(
            "mid",
            "曲".into(),
            Some(&meta(Some(
                "https://y.gtimg.cn/music/photo_new/T002R300x300M000x.jpg",
            ))),
            SongEntities::default(),
        );
        assert_eq!(remote.cover_uri, None, "http 封面不落 UI 直读列（禁网）");
        assert_eq!(
            remote.remote_cover.as_deref(),
            Some("https://y.gtimg.cn/music/photo_new/T002R300x300M000x.jpg")
        );

        let none = song_row_from_qq(
            "mid",
            "曲".into(),
            Some(&meta(None)),
            SongEntities::default(),
        );
        assert_eq!(none.cover_uri, None);
        assert_eq!(none.remote_cover, None);
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

        // 完全无关的名字 → None（本地 miss：空曲目页无信息量且拦远端分流，
        // 退「未找到」/远端装载——D2c）；空参 → None。
        assert!(artist_detail_from(&mut db, "不存在的歌手").is_none());
        assert!(artist_detail_from(&mut db, "").is_none());
    }

    /// 导航参数前缀只认实体 id：`qq:` / 裸展示名 / 非数字不能被误判成本地实体。
    #[test]
    fn entity_param_prefix_parsing() {
        assert_eq!(entity_id_param("album:7", ALBUM_PARAM_PREFIX), Some(7));
        assert_eq!(entity_id_param("artist:12", ARTIST_PARAM_PREFIX), Some(12));
        assert_eq!(entity_id_param("album:0", ALBUM_PARAM_PREFIX), None);
        assert_eq!(entity_id_param("album:x", ALBUM_PARAM_PREFIX), None);
        assert_eq!(entity_id_param("qq:7", ALBUM_PARAM_PREFIX), None);
        assert_eq!(entity_id_param("十一月的萧邦", ALBUM_PARAM_PREFIX), None);
    }

    /// 本地曲目行：歌手/专辑走实体 id 参数（不再用展示名当 id）。
    #[test]
    fn local_song_row_navigates_by_entity_id() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = test_dir("row-entities");
        let a = dir.join("a.flac");
        db.add_local_file(
            &a,
            Some(&meta("夜曲", "周杰伦", "十一月的萧邦", 180_000, "flac")),
        )
        .unwrap();

        let snap = read_snapshot(&mut db).expect("projection ok");
        let row = snap
            .local_tracks
            .iter()
            .find(|r| r.title == "夜曲")
            .expect("local row");
        let artist_id = db
            .library_artists()
            .unwrap()
            .into_iter()
            .find(|g| g.artist == "周杰伦")
            .expect("artist entity")
            .id;
        let album_id = db
            .library_albums(None)
            .unwrap()
            .into_iter()
            .find(|g| g.album == "十一月的萧邦")
            .expect("album entity")
            .id;
        assert_eq!(row.artist_id, artist_id);
        assert_eq!(row.album_id, album_id);

        let track = row.to_track_row();
        assert_eq!(track.artist_mid.to_string(), format!("artist:{artist_id}"));
        assert_eq!(track.album_mid.to_string(), format!("album:{album_id}"));
    }

    /// QQ 曲目行：真 mid → `qq:` 参数（歌手指第一位；资料来源 track_artists /
    /// tracks.album_mid）；实体 id 保持 0。
    #[test]
    fn qq_song_row_navigates_by_mid() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.upsert_track(&hmp_storage::TrackRow {
            source: "qq",
            source_key: "mid-9".into(),
            title: "晴天".into(),
            artist: Some("周杰伦".into()),
            album: Some("叶惠美".into()),
            album_mid: Some("album-mid-1".into()),
            artist_refs: vec![hmp_storage::ArtistRefRow {
                mid: Some("singer-mid-1".into()),
                name: "周杰伦".into(),
            }],
            ..Default::default()
        })
        .unwrap();
        db.add_favorite("qq", "mid-9", "晴天").unwrap();

        let snap = read_snapshot(&mut db).expect("projection ok");
        let row = snap
            .liked
            .iter()
            .find(|r| r.mid == "mid-9")
            .expect("qq row");
        assert_eq!(row.artist_id, 0);
        assert_eq!(row.album_id, 0);
        assert_eq!(row.artist_mid.as_deref(), Some("singer-mid-1"));
        assert_eq!(row.album_mid.as_deref(), Some("album-mid-1"));

        let track = row.to_track_row();
        assert_eq!(track.artist_mid.as_str(), "qq:singer-mid-1");
        assert_eq!(track.album_mid.as_str(), "qq:album-mid-1");
    }

    /// 实体缺失回退裸展示名（历史直插行 / QQ 无 mid），不生成半截前缀。
    #[test]
    fn song_row_nav_falls_back_to_display_names() {
        let mut row = SongRow {
            mid: "local:/a.mp3".into(),
            source: 1,
            title: "A".into(),
            artists: "周杰伦".into(),
            album: "十一月的萧邦".into(),
            artist_id: 0,
            album_id: 0,
            artist_mid: None,
            album_mid: None,
            duration_ms: 0,
            quality: String::new(),
            cover_uri: None,
            remote_cover: None,
            folder: None,
        };
        let track = row.to_track_row();
        assert_eq!(track.artist_mid.as_str(), "周杰伦", "实体缺失回退展示名");
        assert_eq!(track.album_mid.as_str(), "十一月的萧邦");

        // QQ 行无 mid：同样回退展示名（旧按名路径）。
        row.source = 0;
        let track = row.to_track_row();
        assert_eq!(track.artist_mid.as_str(), "周杰伦");
        assert_eq!(track.album_mid.as_str(), "十一月的萧邦");

        // 展示名也为空 → 空串（UI 禁用链接）。
        row.artists = String::new();
        row.album = String::new();
        let track = row.to_track_row();
        assert_eq!(track.artist_mid.as_str(), "");
        assert_eq!(track.album_mid.as_str(), "");
    }

    /// 同名专辑两个实体：`album:<id>` 只出现该实体曲目，行上专辑参数也落到
    /// 各自实体（名称 + 展示歌手能唯一对应时）。
    #[test]
    fn album_detail_by_id_excludes_same_name_other_entity() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = test_dir("album-id");
        let a = dir.join("a.flac");
        let b = dir.join("b.flac");
        db.add_local_file(&a, Some(&meta("A", "歌手甲", "同名专辑", 1_000, "flac")))
            .unwrap();
        db.add_local_file(&b, Some(&meta("B", "歌手乙", "同名专辑", 2_000, "flac")))
            .unwrap();

        let groups = db.library_albums(None).unwrap();
        let album_a = groups
            .iter()
            .find(|g| g.album == "同名专辑" && g.artist.as_deref() == Some("歌手甲"))
            .expect("album A");
        let album_b = groups
            .iter()
            .find(|g| g.album == "同名专辑" && g.artist.as_deref() == Some("歌手乙"))
            .expect("album B");
        assert_ne!(album_a.id, album_b.id, "同名不同专辑歌手是两个实体");

        let detail =
            album_detail_from(&mut db, &format!("album:{}", album_a.id)).expect("album id detail");
        assert_eq!(detail.name, "同名专辑");
        assert_eq!(detail.artist.as_deref(), Some("歌手甲"));
        assert_eq!(detail.tracks.len(), 1, "只含该实体的曲目");
        assert_eq!(detail.tracks[0].title, "A");
        assert_eq!(detail.tracks[0].album_id, album_a.id);
        assert_eq!(
            detail.tracks[0].to_track_row().album_mid.to_string(),
            format!("album:{}", album_a.id)
        );

        // 行级导航参数同样按实体区分（名称 + 展示歌手唯一对应）。
        let snap = read_snapshot(&mut db).expect("projection ok");
        let row_a = snap.local_tracks.iter().find(|r| r.title == "A").unwrap();
        let row_b = snap.local_tracks.iter().find(|r| r.title == "B").unwrap();
        assert_eq!(row_a.album_id, album_a.id);
        assert_eq!(row_b.album_id, album_b.id);

        assert!(album_detail_from(&mut db, "album:999999").is_none());
    }

    /// 同名专辑且行展示歌手对不上专辑歌手（专辑歌手在行级不可见）：
    /// 按实体曲目成员关系精确回填行级 album_id，不退回裸名。
    #[test]
    fn ambiguous_album_row_resolves_by_membership() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = test_dir("album-ambiguous");
        let a = dir.join("a.flac");
        let b = dir.join("b.flac");
        let mut meta_a = meta("A", "演唱者一", "同名专辑", 1_000, "flac");
        meta_a.album_artist = Some("制作人一".into());
        let mut meta_b = meta("B", "演唱者二", "同名专辑", 2_000, "flac");
        meta_b.album_artist = Some("制作人二".into());
        db.add_local_file(&a, Some(&meta_a)).unwrap();
        db.add_local_file(&b, Some(&meta_b)).unwrap();

        let groups = db.library_albums(None).unwrap();
        let album_a = groups
            .iter()
            .find(|g| g.album == "同名专辑" && g.artist.as_deref() == Some("制作人一"))
            .expect("album A");
        let album_b = groups
            .iter()
            .find(|g| g.album == "同名专辑" && g.artist.as_deref() == Some("制作人二"))
            .expect("album B");
        assert_ne!(album_a.id, album_b.id);

        // 行展示歌手（演唱者）与专辑歌手（制作人）不同：名称索引必失配，
        // 成员关系查询把它落回各自的实体。
        let snap = read_snapshot(&mut db).expect("projection ok");
        let row_a = snap.local_tracks.iter().find(|r| r.title == "A").unwrap();
        let row_b = snap.local_tracks.iter().find(|r| r.title == "B").unwrap();
        assert_eq!(row_a.album_id, album_a.id);
        assert_eq!(row_b.album_id, album_b.id);
    }

    /// 歌手实体 id 路径不再走"最长包含匹配"：展示串里的 HOYO-MiX 不抢页。
    #[test]
    fn artist_detail_by_id_uses_entity_without_name_heuristics() {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let dir = test_dir("artist-id");
        let a = dir.join("a.flac");
        let b = dir.join("b.flac");
        db.add_local_file(
            &a,
            Some(&meta(
                "歌一",
                "知更鸟 / HOYO-MiX / Chevy",
                "合辑一",
                100_000,
                "flac",
            )),
        )
        .unwrap();
        db.add_local_file(
            &b,
            Some(&meta("歌二", "HOYO-MiX", "合辑二", 120_000, "flac")),
        )
        .unwrap();

        let groups = db.library_artists().unwrap();
        let literal = groups
            .iter()
            .find(|g| g.artist == "知更鸟 / HOYO-MiX / Chevy")
            .expect("literal artist entity");
        let detail = artist_detail_from(&mut db, &format!("artist:{}", literal.id))
            .expect("artist id detail");
        assert_eq!(detail.name, "知更鸟 / HOYO-MiX / Chevy");
        assert_eq!(detail.tracks.len(), 1, "只含该实体曲目");
        assert_eq!(detail.tracks[0].title, "歌一");
        assert_eq!(detail.tracks[0].artist_id, literal.id);
        assert_eq!(detail.albums.len(), 1);
        assert!(
            detail.albums[0].param.starts_with(ALBUM_PARAM_PREFIX),
            "歌手页专辑卡按实体 id 导航: {}",
            detail.albums[0].param
        );

        assert!(artist_detail_from(&mut db, "artist:999999").is_none());
    }
}
