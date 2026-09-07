//! UI 桥接：媒体库快照装载为 Slint 模型 + 导航/主题/音质/过滤回调绑定。
//! （旧版对接 AppCore 的桥随旧 UI 契约废弃，M8 数据接线时重写回来；
//! 页面数据源 = [`library_view`] 直读 library.sqlite3，播放/命令桥另行接线。）

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, ModelRc, SharedString, Weak};

use crate::covers::cover_image;
use crate::format::{format_bytes, format_cny};
use crate::library_view::{self, PlaylistEntry, SongRow};
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

/// 导航 / 主题 / 音质回调绑定（历史栈 + 偏好持久化）
pub fn bind(ui: &AppWindow, prefs: Arc<Mutex<Prefs>>) {
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

    // ——— 音质偏好 ———
    {
        let prefs = Arc::clone(&prefs);
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        Quality::get(ui).on_select(move |tier| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            Quality::get(&ui).set_selected(tier);
            prefs.lock().expect("prefs").quality = tier;
            crate::prefs::store(&prefs.lock().expect("prefs").clone());
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
}
