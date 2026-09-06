//! UI 桥接：mock 数据装载为 Slint 模型 + 导航/主题/音质/过滤回调绑定。
//! （旧版对接 AppCore 的桥随旧 UI 契约废弃，M8 数据接线时重写回来。）

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, ModelRc, SharedString, Weak};

use crate::covers::cover_image;
use crate::format::{format_bytes, format_cny, format_count_wan, format_long_duration};
use crate::mock;
use crate::prefs::Prefs;
use crate::{
    AppWindow, CoverCardData, Data, FolderRow, Nav, Quality, Theme, TrackRow,
};

const RECENT_PREVIEW_SIZE: usize = 5;
const RECENT_PAGE_SIZE: usize = 12;
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

fn playlist_card(playlist: &mock::PlaylistRef) -> CoverCardData {
    cover_card(
        &playlist.id,
        &playlist.name,
        format!("{}次播放", format_count_wan(playlist.play_count)),
        &format!("playlist:{}", playlist.id),
    )
}

fn track_row_of(track: &mock::LocalTrack) -> TrackRow {
    mock::to_track_row(&track.song)
}

/// "2026-09-05 21:30" → "09-05 21:30"（年份归页头层级）
fn short_scan_time(iso: &str) -> &str {
    iso.get(5..).unwrap_or(iso)
}

fn path_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 把 mock 数据装载进 Data global（mock-first：页面只读模型与统计字段）
pub fn load_data(ui: &AppWindow) {
    let data = Data::get(ui);
    let pool = mock::song_pool();
    let albums = mock::curated_albums();

    let liked = mock::liked_songs(&pool);
    let recent_preview: Vec<mock::SongRef> = pool.iter().take(RECENT_PREVIEW_SIZE).cloned().collect();
    let recent = mock::recent_records(&pool, RECENT_PAGE_SIZE);
    let local = mock::local_library(&pool);
    let downloads = mock::download_library(&pool);
    let purchased = mock::purchased_music(&pool, &albums);
    let created = mock::created_playlist_refs();
    let favorited = mock::favorited_playlist_refs();

    data.set_liked(model(mock::track_rows(&liked)));
    data.set_liked_count(liked.len() as i32);
    data.set_playlist_count((created.len() + favorited.len()) as i32);
    data.set_created_playlists(model(created.iter().map(playlist_card).collect::<Vec<_>>()));
    data.set_favorited_playlists(model(favorited.iter().map(playlist_card).collect::<Vec<_>>()));
    data.set_recent_preview(model(mock::track_rows(&recent_preview)));

    let recent_songs: Vec<mock::SongRef> = recent.iter().map(|record| record.song.clone()).collect();
    data.set_recent(model(mock::track_rows(&recent_songs)));
    data.set_recent_count(recent.len() as i32);
    data.set_recent_latest(
        recent
            .first()
            .map(|record| record.label.clone())
            .unwrap_or_else(|| "今天".into())
            .into(),
    );
    data.set_recent_earliest(
        recent
            .last()
            .map(|record| record.label.clone())
            .unwrap_or_else(|| "今天".into())
            .into(),
    );

    let local_total_size: u64 = local.tracks.iter().map(|track| track.size_bytes).sum();
    let local_total_duration: u64 = local.tracks.iter().map(|track| track.song.duration_ms).sum();
    let all_local_rows: Vec<TrackRow> = local.tracks.iter().map(track_row_of).collect();
    data.set_local_tracks(model(all_local_rows.clone()));
    data.set_local_count(local.tracks.len() as i32);
    data.set_local_duration(format_long_duration(local_total_duration).into());
    data.set_local_size(format_bytes(local_total_size).into());
    data.set_folders(model(
        local
            .folders
            .iter()
            .map(|folder| FolderRow {
                path: folder.path.clone().into(),
                track_count: folder.track_count as i32,
                size_text: format_bytes(folder.size_bytes).into(),
                last_scan: short_scan_time(&folder.last_scan_at).into(),
            })
            .collect::<Vec<_>>(),
    ));

    // ——— 监视文件夹过滤：点击行过滤曲目表，再点/显示全部取消 ———
    // TrackRow 不带 folder 字段（派生关系留在 Rust 侧），这里并行持有分组归属。
    let rows_by_folder: Vec<(String, TrackRow)> = local
        .tracks
        .iter()
        .map(|track| (track.folder.clone(), track_row_of(track)))
        .collect();
    let folder_paths: Vec<String> = local.folders.iter().map(|f| f.path.clone()).collect();
    let folder_titles: Vec<String> = local
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
            let selected = if index == data.get_selected_folder() { -1 } else { index };
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

    let downloads_size: u64 = downloads.tracks.iter().map(|track| track.size_bytes).sum();
    let lossless = downloads
        .tracks
        .iter()
        .filter(|track| track.format == "FLAC")
        .count();
    data.set_downloads(model(downloads.tracks.iter().map(track_row_of).collect::<Vec<_>>()));
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
                    format!("{} 首 · {} 购买", entry.album.songs.len(), entry.purchased_at),
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
    fn short_scan_time_strips_year() {
        assert_eq!(short_scan_time("2026-09-05 21:30"), "09-05 21:30");
    }

    #[test]
    fn basename() {
        assert_eq!(path_basename("~/Music/无损收藏"), "无损收藏");
        assert_eq!(path_basename("~/Music/胡桃音乐"), "胡桃音乐");
    }
}
