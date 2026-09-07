//! UI 桥接：媒体库快照装载为 Slint 模型 + 导航/主题/音质/过滤回调绑定。
//! （旧版对接 AppCore 的桥随旧 UI 契约废弃，M8 数据接线时重写回来；
//! 页面数据源 = [`library_view`] 直读 library.sqlite3，播放/命令桥另行接线。）

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, ModelRc, SharedString, Weak};

use crate::covers::cover_image;
use crate::format::{format_bytes, format_cny, format_long_duration};
use crate::library_view::{self, PlaylistEntry, SongRow, local_cover_image};
use crate::mock;
use crate::prefs::Prefs;
use crate::{
    AppWindow, CoverCardData, Data, FolderRow, Nav, PlaylistCover, Quality, Theme, TrackRow,
};

const RECENT_PREVIEW_SIZE: usize = 5;
const NAV_HISTORY_CAP: usize = 50;

fn model<T: 'static + Clone>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

use slint::VecModel;

fn cover_card(mid: &str, title: &str, subtitle: String, cover_seed: &str) -> CoverCardData {
    CoverCardData {
        mid: mid.into(),
        title: title.into(),
        subtitle: subtitle.into(),
        cover: cover_image(cover_seed),
    }
}

/// 歌单卡（副标题来自媒体库投影的 "N 首"——媒体库无播放计数，不伪造"X次播放"）
fn playlist_card(entry: &PlaylistEntry) -> CoverCardData {
    cover_card(
        &entry.id,
        &entry.name,
        entry.subtitle.clone(),
        &format!("playlist:{}", entry.id),
    )
}

/// 侧栏歌单渐变：媒体库无配色数据，色对是 UI 层的确定性装饰——
/// 8 组手选低饱和对，按歌单 id 的 FNV-1a 哈希选取（与封面同一确定性地基）。
fn sidebar_cover(entry: &PlaylistEntry) -> PlaylistCover {
    const PAIRS: [(u8, u8, u8, u8, u8, u8); 8] = [
        (0xe4, 0x4b, 0x32, 0xf3, 0xb3, 0x2f),
        (0x1e, 0x38, 0x5f, 0xd4, 0x9b, 0x60),
        (0x76, 0x60, 0xa4, 0xef, 0x9d, 0x9d),
        (0x2f, 0x6b, 0x4f, 0xa8, 0xd0, 0x8d),
        (0x3a, 0x5a, 0x7c, 0x9f, 0xc2, 0xd9),
        (0xb0, 0x7d, 0x2b, 0xe8, 0xd0, 0x8d),
        (0x4a, 0x4a, 0x66, 0xb3, 0xb3, 0xd9),
        (0x8c, 0x3b, 0x58, 0xe8, 0xa0, 0xb0),
    ];
    let hash = crate::covers::hash_seed(&format!("playlist:{}", entry.id)) as usize;
    let (r1, g1, b1, r2, g2, b2) = PAIRS[hash % PAIRS.len()];
    PlaylistCover {
        name: entry.name.clone().into(),
        c1: slint::Color::from_rgb_u8(r1, g1, b1),
        c2: slint::Color::from_rgb_u8(r2, g2, b2),
    }
}

fn path_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// UI 音质档（0=标准 1=高清 2=无损 3=Hi-Res）→ config 别名（写路径）。
/// Hi-Res 档映射 `hires`：resolver 对 hires/master 请求同一 MASTER 文件
/// 类型（上游无独立 Hi-Res 档，见 daemon player.rs 映射注释）。
fn quality_tier_alias(tier: i32) -> Option<&'static str> {
    match tier {
        0 => Some("128"),
        1 => Some("320"),
        2 => Some("flac"),
        3 => Some("hires"),
        _ => None,
    }
}

/// config 别名 → UI 音质档（读路径）。`auto`/未知 → None：保持 UI 现选，
/// 不把非档位语义伪造成某一档。
fn quality_alias_tier(mode: &str) -> Option<i32> {
    match mode {
        "128" => Some(0),
        "320" | "aac" => Some(1),
        "flac" => Some(2),
        "hires" | "master" | "atmos" => Some(3),
        _ => None,
    }
}

/// 搜索结果行（smartbox 窄投影：无专辑/时长 → 对应列收起，mid 程序化封面）。
fn search_track_row(song: &hmp_core::SearchSong) -> TrackRow {
    TrackRow {
        mid: song.mid.as_str().into(),
        source: 0,
        title: song.name.as_str().into(),
        artists: song.singer.as_str().into(),
        artist_mid: "".into(),
        album: "".into(),
        album_mid: "".into(),
        duration_ms: 0,
        quality: "".into(),
        cover: crate::covers::cover_image(&format!("album:{}", song.mid)),
    }
}

/// 把媒体库快照（直读 library.sqlite3，离线降级为空）装载进 Data global：
/// 五个库页真数据；下载/已购两页后端无对应域，保留 mock 喂数据。
pub fn load_data(ui: &AppWindow) {
    let data = Data::get(ui);
    let snap = library_view::load_snapshot();

    // ——— 我喜欢 ———
    data.set_liked(model(
        snap.liked
            .iter()
            .map(SongRow::to_track_row)
            .collect::<Vec<_>>(),
    ));
    data.set_liked_count(snap.liked.len() as i32);

    // ——— 歌单（relation 分流：local/owned → 自建，subscribed → 收藏）———
    let created_cards: Vec<CoverCardData> = snap.created.iter().map(playlist_card).collect();
    let favorited_cards: Vec<CoverCardData> = snap.favorited.iter().map(playlist_card).collect();
    data.set_playlist_count((created_cards.len() + favorited_cards.len()) as i32);
    data.set_created_playlists(model(created_cards));
    data.set_favorited_playlists(model(favorited_cards));
    // 侧栏歌单区（宽侧栏分组/窄边栏二级共用）
    data.set_sidebar_created(model(
        snap.created.iter().map(sidebar_cover).collect::<Vec<_>>(),
    ));
    data.set_sidebar_favorited(model(
        snap.favorited.iter().map(sidebar_cover).collect::<Vec<_>>(),
    ));

    // ——— 最近播放（预览 = 我喜欢页前 5；latest/earliest 已按真实时间戳格式化）———
    data.set_recent_preview(model(
        snap.recent
            .iter()
            .take(RECENT_PREVIEW_SIZE)
            .map(SongRow::to_track_row)
            .collect::<Vec<_>>(),
    ));
    data.set_recent(model(
        snap.recent
            .iter()
            .map(SongRow::to_track_row)
            .collect::<Vec<_>>(),
    ));
    data.set_recent_count(snap.recent.len() as i32);
    data.set_recent_latest(snap.recent_latest.clone().into());
    data.set_recent_earliest(snap.recent_earliest.clone().into());

    // ——— 音乐库（本地）：曲目表 + 扫描根聚合统计 ———
    let all_local_rows: Vec<TrackRow> = snap
        .local_tracks
        .iter()
        .map(SongRow::to_track_row)
        .collect();
    data.set_local_tracks(model(all_local_rows.clone()));
    data.set_local_count(snap.local_tracks.len() as i32);
    data.set_local_duration(snap.local_duration_text.clone().into());
    data.set_local_size(snap.local_size_text.clone().into());
    data.set_folders(model(
        snap.folders
            .iter()
            .map(|folder| FolderRow {
                path: folder.path.clone().into(),
                track_count: folder.track_count,
                size_text: format_bytes(folder.size_bytes).into(),
                last_scan: folder.last_scan.clone().into(),
            })
            .collect::<Vec<_>>(),
    ));

    // ——— 监视文件夹过滤：点击行过滤曲目表，再点/显示全部取消 ———
    // TrackRow 不带 folder 字段（派生关系留在 Rust 侧）；分组来自真实扫描根
    // 前缀匹配（嵌套根取最长优先），与文件夹行的聚合口径一致。
    let rows_by_folder: Vec<(String, TrackRow)> = snap
        .local_tracks
        .iter()
        .filter_map(|row| {
            let folder = row.folder.clone()?;
            Some((folder, row.to_track_row()))
        })
        .collect();
    let folder_paths: Vec<String> = snap.folders.iter().map(|f| f.path.clone()).collect();
    let folder_titles: Vec<String> = snap
        .folders
        .iter()
        .map(|f| format!("{} · {} 首", path_basename(&f.path), f.track_count))
        .collect();
    {
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        data.on_filter_folder(move |index| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let data = Data::get(&ui);
            let selected = if index == data.get_selected_folder() {
                -1
            } else {
                index
            };
            data.set_selected_folder(selected);

            let (rows, title): (Vec<TrackRow>, SharedString) = if selected < 0 {
                (all_local_rows.clone(), "全部音乐".into())
            } else if let Some(path) = folder_paths.get(selected as usize) {
                let filtered = rows_by_folder
                    .iter()
                    .filter(|(folder, _)| folder == path)
                    .map(|(_, row)| row.clone())
                    .collect();
                (filtered, folder_titles[selected as usize].clone().into())
            } else {
                (all_local_rows.clone(), "全部音乐".into())
            };
            data.set_local_tracks(model(rows));
            data.set_local_table_title(title);
        });
    }

    // ——— 下载/已购两页：后端无下载/已购域，暂 mock（缺口见 docs/AUDIT.md M8 记录）———
    let pool = mock::song_pool();
    let albums = mock::curated_albums();
    let downloads = mock::download_library(&pool);
    let purchased = mock::purchased_music(&pool, &albums);

    let downloads_size: u64 = downloads.tracks.iter().map(|track| track.size_bytes).sum();
    let lossless = downloads
        .tracks
        .iter()
        .filter(|track| track.format == "FLAC")
        .count();
    data.set_downloads(model(
        downloads
            .tracks
            .iter()
            .map(|track| mock::to_track_row(&track.song))
            .collect::<Vec<_>>(),
    ));
    data.set_downloads_count(downloads.tracks.len() as i32);
    data.set_downloads_size(format_bytes(downloads_size).into());
    data.set_downloads_lossless(lossless as i32);
    data.set_downloads_storage_path(downloads.storage_path.clone().into());

    let purchased_songs: Vec<mock::SongRef> = purchased
        .singles
        .iter()
        .map(|single| single.song.clone())
        .collect();
    data.set_purchased_singles(model(mock::track_rows(&purchased_songs)));
    let total_fen: u64 = purchased
        .singles
        .iter()
        .map(|single| single.price_fen)
        .chain(purchased.albums.iter().map(|album| album.price_fen))
        .sum();
    data.set_purchased_single_count(purchased.singles.len() as i32);
    data.set_purchased_album_count(purchased.albums.len() as i32);
    data.set_purchased_total(format_cny(total_fen).into());
    data.set_purchased_albums(model(
        purchased
            .albums
            .iter()
            .map(|entry| {
                cover_card(
                    &entry.album.mid,
                    &entry.album.name,
                    format!(
                        "{} 首 · {} 购买",
                        entry.album.songs.len(),
                        entry.purchased_at
                    ),
                    &format!("album:{}", entry.album.mid),
                )
            })
            .collect::<Vec<_>>(),
    ));
}

/// 库变更刷新（`Event::LibraryChanged` 驱动，AUDIT §8.9）：重装载库页数据
/// 并重放当前路由的详情装载。刷新失败/库缺失时 load_data 自行降级空态。
pub fn refresh(ui: &AppWindow) {
    load_data(ui);
    let nav = Nav::get(ui);
    apply_route(ui, nav.get_route(), nav.get_param());
}

/// 路由落地后的详情装载：歌单/专辑/歌手三页按参数查库填充 Data
/// （同步读，单页查询量小；其他路由无详情数据）。found=false → 页面诚实空态。
fn apply_route(ui: &AppWindow, route: crate::Route, param: SharedString) {
    match route {
        crate::Route::Playlist => load_playlist_detail(ui, &param),
        crate::Route::Album => load_album_detail(ui, &param),
        crate::Route::Artist => load_artist_detail(ui, &param),
        _ => {}
    }
}

fn load_playlist_detail(ui: &AppWindow, param: &str) {
    let data = Data::get(ui);
    let detail = param
        .parse::<i64>()
        .ok()
        .and_then(library_view::playlist_detail);
    match detail {
        Some(detail) => {
            data.set_playlist_found(true);
            data.set_playlist_name(detail.name.into());
            data.set_playlist_meta_items(model(vec![
                format!("{} 首", detail.tracks.len()).into(),
                format!("总时长 {}", format_long_duration(detail.total_ms)).into(),
            ]));
            // 与库页歌单卡同一 seed（playlist:{id}），实体身份一致
            data.set_playlist_cover(cover_image(&format!("playlist:{param}")));
            data.set_playlist_tracks(model(
                detail.tracks.iter().map(SongRow::to_track_row).collect(),
            ));
        }
        None => data.set_playlist_found(false),
    }
}

fn load_album_detail(ui: &AppWindow, param: &str) {
    let data = Data::get(ui);
    match library_view::album_detail(param) {
        Some(detail) => {
            data.set_album_found(true);
            data.set_album_name(detail.name.clone().into());
            data.set_album_artist(detail.artist.unwrap_or_default().into());
            let mut meta = vec![
                // 年份无数据源 → "—"（不伪造发行时间）
                detail
                    .year
                    .map(|y| y.to_string())
                    .unwrap_or_else(|| "—".into()),
                format!("{} 首", detail.tracks.len()),
                format!("总时长 {}", format_long_duration(detail.total_ms)),
            ];
            meta.push("本地媒体库".into());
            data.set_album_meta_items(model(
                meta.into_iter().map(SharedString::from).collect::<Vec<_>>(),
            ));
            data.set_album_cover(
                local_cover_image(detail.cover_uri.as_deref())
                    .unwrap_or_else(|| cover_image(&format!("album:{}", detail.name))),
            );
            data.set_album_tracks(model(
                detail.tracks.iter().map(SongRow::to_track_row).collect(),
            ));
        }
        None => data.set_album_found(false),
    }
}

fn load_artist_detail(ui: &AppWindow, param: &str) {
    let data = Data::get(ui);
    match library_view::artist_detail(param) {
        Some(detail) => {
            data.set_artist_found(true);
            data.set_artist_name(detail.name.clone().into());
            data.set_artist_meta_items(model(vec![
                format!("{} 首歌曲", detail.tracks.len()).into(),
                format!("{} 张专辑", detail.albums.len()).into(),
                "本地媒体库".into(),
            ]));
            // 与 Vue 版同款 seed（artist:{name}），同实体各处同图
            data.set_artist_photo(cover_image(&format!("artist:{}", detail.name)));
            data.set_artist_tracks(model(
                detail.tracks.iter().map(SongRow::to_track_row).collect(),
            ));
            data.set_artist_albums(model(
                detail
                    .albums
                    .iter()
                    .map(|album| CoverCardData {
                        mid: album.name.clone().into(),
                        title: album.name.clone().into(),
                        subtitle: format!("{} · {} 首", album.year_text, album.track_count).into(),
                        // 真封面优先（与专辑页同源），缺失回退程序化封面
                        cover: local_cover_image(album.cover_uri.as_deref())
                            .unwrap_or_else(|| cover_image(&format!("album:{}", album.name))),
                    })
                    .collect::<Vec<_>>(),
            ));
        }
        None => data.set_artist_found(false),
    }
}

/// 导航 / 主题 / 音质 / 搜索 / 账号回调绑定（历史栈 + 偏好持久化 + IPC 出网）
pub fn bind(
    ui: &AppWindow,
    prefs: Arc<Mutex<Prefs>>,
    runtime: Arc<crate::backend::BackendRuntime>,
) {
    // ——— 导航历史栈 ———
    let nav = Nav::get(ui);
    let history: Arc<Mutex<Vec<(crate::Route, SharedString)>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let history = Arc::clone(&history);
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        nav.on_navigate(move |route, param| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let nav = Nav::get(&ui);
            let current = (nav.get_route(), nav.get_param());
            {
                let mut stack = history.lock().expect("nav history");
                // 同页重复导航不入栈；栈满丢最旧
                if stack.last() != Some(&current) && Some(&current) != stack.first() {
                    if stack.len() >= NAV_HISTORY_CAP {
                        stack.remove(0);
                    }
                    stack.push(current);
                }
            }
            nav.set_route(route);
            nav.set_param(param);
            nav.set_can_go_back(!history.lock().expect("nav history").is_empty());
            apply_route(&ui, nav.get_route(), nav.get_param());
        });
    }
    {
        let history = Arc::clone(&history);
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        nav.on_back(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let nav = Nav::get(&ui);
            let previous = history.lock().expect("nav history").pop();
            if let Some((route, param)) = previous {
                nav.set_route(route);
                nav.set_param(param);
            }
            nav.set_can_go_back(!history.lock().expect("nav history").is_empty());
            apply_route(&ui, nav.get_route(), nav.get_param());
        });
    }

    // ——— 主题循环（跟随系统 → 浅色 → 深色 → …）———
    {
        let prefs = Arc::clone(&prefs);
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        Theme::get(ui).on_cycle_mode(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let theme = Theme::get(&ui);
            let next = (theme.get_mode() + 1) % 3;
            theme.set_mode(next);
            prefs.lock().expect("prefs").theme_mode = next;
            crate::prefs::store(&prefs.lock().expect("prefs").clone());
        });
    }

    // ——— 主题直接设档（设置页三选）———
    {
        let prefs = Arc::clone(&prefs);
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        Theme::get(ui).on_set_mode(move |mode| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            Theme::get(&ui).set_mode(mode);
            prefs.lock().expect("prefs").theme_mode = mode;
            crate::prefs::store(&prefs.lock().expect("prefs").clone());
        });
    }

    // ——— 音质偏好 ———
    // daemon config.toml 是唯一事实源（AUDIT §8.7）：选择即写 IPC，本地
    // prefs 只作离线启动的展示兜底；启动时从 daemon 同步一次（CLI 写入
    // 的变化桌面也可见）。
    {
        let prefs = Arc::clone(&prefs);
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let runtime = Arc::clone(&runtime);
        Quality::get(ui).on_select(move |tier| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            Quality::get(&ui).set_selected(tier);
            prefs.lock().expect("prefs").quality = tier;
            crate::prefs::store(&prefs.lock().expect("prefs").clone());
            if let Some(mode) = quality_tier_alias(tier) {
                let request = hmp_core::Request::QualitySet {
                    mode: mode.into(),
                    fallback: true,
                };
                runtime.spawn(async move {
                    let _ = crate::backend::request(request).await;
                });
            }
        });
    }
    {
        // 启动同步：daemon 侧偏好 → UI 选中档（auto/未知别名保持现选，
        // 不伪造档位展示）。
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        runtime.spawn(async move {
            let result = crate::backend::request(hmp_core::Request::QualityGet).await;
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                if let Ok(hmp_core::Response::Quality(pref)) = result {
                    if let Some(tier) = quality_alias_tier(&pref.mode) {
                        Quality::get(&ui).set_selected(tier);
                    }
                }
            });
        });
    }

    // ——— 搜索（daemon Search IPC；免登录 smartbox）———
    {
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let runtime = Arc::clone(&runtime);
        Data::get(ui).on_search(move |query| {
            let keyword = query.trim().to_string();
            if keyword.is_empty() {
                return;
            }
            if let Some(ui) = ui_weak.upgrade() {
                let data = Data::get(&ui);
                data.set_search_query(SharedString::from(keyword.clone()));
                data.set_search_state(1);
            }
            let ui_weak = ui_weak.clone();
            runtime.spawn(async move {
                let result = crate::backend::request(hmp_core::Request::Search { keyword }).await;
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak.upgrade() else {
                        return;
                    };
                    let data = Data::get(&ui);
                    match result {
                        Ok(hmp_core::Response::Search(page)) => {
                            let rows: Vec<TrackRow> =
                                page.songs.iter().map(search_track_row).collect();
                            let found = !rows.is_empty();
                            data.set_search_count(rows.len() as i32);
                            data.set_search_results(model(rows));
                            data.set_search_state(if found { 2 } else { 3 });
                        }
                        // 离线/daemon 错误/协议错误统一失败态（页面文案覆盖）。
                        _ => data.set_search_state(4),
                    }
                });
            });
        });
    }

    // ——— 账号状态（daemon AccountStatus IPC；设置页账号面板）———
    // daemon 冷启动（连接失败）时重试：首次连接由订阅循环 connect_or_spawn
    // 拉起 daemon 需 ~1-3s，这里的小重试覆盖该窗口；彻底离线 → 专属失败态
    // （区别于"未登录"，不误导用户去重新登录）。
    {
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        runtime.spawn(async move {
            let mut result = Err(crate::backend::BackendError::NoBackendBinary);
            for _ in 0..4 {
                result = crate::backend::request(hmp_core::Request::AccountStatus).await;
                if matches!(result, Ok(hmp_core::Response::AccountStatus(_))) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
            let online = matches!(result, Ok(hmp_core::Response::AccountStatus(_)));
            let apply = move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let data = Data::get(&ui);
                if let Ok(hmp_core::Response::AccountStatus(info)) = result {
                    data.set_account_logged_in(info.logged_in);
                    data.set_account_nickname(info.nickname.clone().into());
                    data.set_account_uin(info.uin.clone().into());
                    data.set_account_vip(info.vip_summary.clone().into());
                }
                data.set_account_state(if online { 1 } else { 2 });
            };
            let _ = slint::invoke_from_event_loop(apply);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename() {
        assert_eq!(path_basename("~/Music/无损收藏"), "无损收藏");
        assert_eq!(path_basename("~/Music/胡桃音乐"), "胡桃音乐");
    }

    #[test]
    fn sidebar_cover_is_deterministic() {
        let entry = PlaylistEntry {
            id: "3".into(),
            name: "深夜循环".into(),
            subtitle: "12 首".into(),
        };
        let a = sidebar_cover(&entry);
        let b = sidebar_cover(&entry);
        assert_eq!(a.c1, b.c1);
        assert_eq!(a.c2, b.c2);
        assert_eq!(a.name, "深夜循环");
    }

    /// 档位 ↔ config 别名往返：写路径四个档位各有别名，读路径能映射回来。
    #[test]
    fn quality_tier_alias_roundtrip() {
        for tier in 0..=3 {
            let alias = quality_tier_alias(tier).expect("四档都有别名");
            assert_eq!(quality_alias_tier(alias), Some(tier), "alias={alias}");
        }
        assert_eq!(quality_tier_alias(9), None);
        // 读路径兼容 CLI 可写的其余别名；auto/未知不映射（保持 UI 现选）。
        assert_eq!(quality_alias_tier("aac"), Some(1));
        assert_eq!(quality_alias_tier("master"), Some(3));
        assert_eq!(quality_alias_tier("atmos"), Some(3));
        assert_eq!(quality_alias_tier("auto"), None);
        assert_eq!(quality_alias_tier("bogus"), None);
    }

    #[test]
    fn search_track_row_maps_song_fields() {
        let song = hmp_core::SearchSong {
            mid: "0039MnYb0qxYhV".into(),
            name: "夜曲".into(),
            singer: "周杰伦".into(),
        };
        let row = search_track_row(&song);
        assert_eq!(row.mid, "0039MnYb0qxYhV");
        assert_eq!(row.source, 0);
        assert_eq!(row.title, "夜曲");
        assert_eq!(row.artists, "周杰伦");
        assert_eq!(row.album, "");
        assert_eq!(row.duration_ms, 0);
        assert!(row.cover.size().width > 0, "封面程序化占位非空");
    }
}
