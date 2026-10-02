//! 列表曲目封面预取（2026-10-03 封面审计第四断点）：列表落地即批量取图。
//!
//! 此前只有 discover 歌单卡（online_covers）与当前播放曲（player_bridge）有
//! 异步补图；歌曲行（歌单详情/我喜欢/榜单……）永远程序化占位。预取链路：
//!
//! UI 列表落地 → (mid, url) 清单 → daemon `CoverGet`（v8 `cover_cache` 盘级
//! 索引优先，同 URL 全生命周期只下载一次）→ `file://` 本地产物 → 按行回填
//! Image；daemon 侧同时 `rebind_cover_url` 把库内远程 URL 升级为本地产物，
//! 下一轮投影直读盘、零 IPC。
//!
//! 去重/竞态纪律（online_covers / player_bridge 同款）：
//! - `REQUESTED`（mid|url 全局）：进程内同图只发一次，失败不重试
//!   （下次进页面由 daemon 盘缓存毫秒回，无需重试逻辑）；
//! - `COVER_IMAGES`（url → Image）：命中直接同步回填——刷新重建模型后
//!   REQUESTED 已挡住重发，靠这层让新模型行立即拿到图（discover 刷新回退
//!   占位的已知缺陷在此不复现）；
//! - 回包广播所有曲目列表模型：同一曲目出现在多个列表（我喜欢 + 歌单详情）
//!   时在途竞态也一并覆盖；行已滚出模型（导航重建）则匹配不到，自然丢弃。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use slint::{ComponentHandle, Global, Image, Model};

use crate::{AppWindow, Data, TrackRow};

/// 并发上限：daemon 命中盘索引时极快，未命中时打满 CDN 的在途请求数也
/// 不至于触发限流；大歌单（2000 首）分钟级渐进填满，全程不阻塞 UI。
const MAX_INFLIGHT: usize = 8;

thread_local! {
    /// 已发起过的 mid|url（完成或失败均不重发，进程级）。
    static REQUESTED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// 远程 URL → 已取回的 Image（跨导航/刷新复用，进程级）。
    static COVER_IMAGES: RefCell<HashMap<String, Image>> = RefCell::new(HashMap::new());
    /// 读盘缓存的路径 → Image（load_from_path 有 IO，占位回填高频命中）。
    static IMAGE_CACHE: RefCell<HashMap<String, Image>> = RefCell::new(HashMap::new());
}

/// 预取目标：曲目列表模型（Data global 属性）。回填广播时逐一遍历。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrackSlot {
    Liked,
    Recent,
    Playlist,
    Album,
    Artist,
    DiscoverNewSongs,
    TopDetail,
    Guess,
}

impl TrackSlot {
    /// 全部曲目列表（广播面；queue/播放页全局、local-tracks/downloads 全本地
    /// 行无远程封面，不在列）。
    pub fn all() -> &'static [TrackSlot] {
        &[
            TrackSlot::Liked,
            TrackSlot::Recent,
            TrackSlot::Playlist,
            TrackSlot::Album,
            TrackSlot::Artist,
            TrackSlot::DiscoverNewSongs,
            TrackSlot::TopDetail,
            TrackSlot::Guess,
        ]
    }

    fn model_of(&self, ui: &AppWindow) -> Option<slint::ModelRc<TrackRow>> {
        let data = Data::get(ui);
        Some(match self {
            TrackSlot::Liked => data.get_liked(),
            TrackSlot::Recent => data.get_recent(),
            TrackSlot::Playlist => data.get_playlist_tracks(),
            TrackSlot::Album => data.get_album_tracks(),
            TrackSlot::Artist => data.get_artist_tracks(),
            TrackSlot::DiscoverNewSongs => data.get_discover_new_songs(),
            TrackSlot::TopDetail => data.get_top_detail_tracks(),
            TrackSlot::Guess => data.get_guess_tracks(),
        })
    }
}

/// 回填：全 slot 扫描同 mid 行，命中即覆盖封面（model_of 返回的 ModelRc
/// 与 Data 属性同源——set_row_data 即时生效）。
fn apply_cover(ui: &AppWindow, mid: &str, image: &Image) {
    for slot in TrackSlot::all() {
        let Some(model) = slot.model_of(ui) else {
            continue;
        };
        for i in 0..model.row_count() {
            let mut row = match model.row_data(i) {
                Some(r) => r,
                None => continue,
            };
            if row.mid.as_str() == mid {
                row.cover = image.clone();
                model.set_row_data(i, row);
            }
        }
    }
}

/// 读盘加载（路径缓存；文件被删/损坏返回 None，本次不回填）。
fn load_image(path: &str) -> Option<Image> {
    IMAGE_CACHE.with(|cache| {
        if let Some(hit) = cache.borrow().get(path) {
            return Some(hit.clone());
        }
        let image = Image::load_from_path(std::path::Path::new(path)).ok()?;
        cache.borrow_mut().insert(path.to_owned(), image.clone());
        Some(image)
    })
}

/// 列表落地后的预取入口（事件循环线程调用；apply_detail / apply_snapshot /
/// 在线页装载点同处）。
///
/// `targets`：(mid, 远程封面 URL) 对——QQ 行的 `remote_cover` 投影，或在线页
/// 行的 picurl。旁路命中的立即同步回填；其余限流批量 CoverGet。
pub fn prefetch_tracks(
    ui: &AppWindow,
    runtime: &Arc<crate::backend::BackendRuntime>,
    targets: Vec<(String, String)>,
) {
    if targets.is_empty() {
        return;
    }
    let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_INFLIGHT));
    let ui_weak = ui.as_weak();
    let mut dispatched = 0usize;
    for (mid, url) in targets {
        if mid.is_empty() || url.is_empty() {
            continue;
        }
        // 旁路命中（此前取回过）：直接同步回填，不走网络路径
        let cached = COVER_IMAGES.with(|cache| cache.borrow().get(&url).cloned());
        if let Some(image) = cached {
            apply_cover(ui, &mid, &image);
            continue;
        }
        // 进程内去重：同图只发一次（完成或失败均不重发）
        let key = format!("{mid}|{url}");
        if REQUESTED.with(|set| !set.borrow_mut().insert(key)) {
            continue;
        }
        let permits = semaphore.clone();
        let runtime = runtime.clone();
        let ui_weak = ui_weak.clone();
        runtime.spawn(async move {
            let _permit = permits.acquire_owned().await.expect("semaphore open");
            let Ok(hmp_core::Response::Cover(uri)) =
                crate::backend::request(hmp_core::Request::CoverGet { url: url.clone() }).await
            else {
                return; // 失败不重试（daemon 盘缓存使下次进页面自然重试）
            };
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return; // 窗口已销毁
                };
                let path = crate::covers::file_uri_to_path(&uri).unwrap_or(uri);
                let Some(image) = load_image(&path) else {
                    return;
                };
                COVER_IMAGES.with(|cache| cache.borrow_mut().insert(url.clone(), image.clone()));
                apply_cover(&ui, &mid, &image);
            });
        });
        dispatched += 1;
    }
    if dispatched > 0 {
        tracing::debug!(dispatched, "track covers prefetch dispatched");
    }
}

/// SongRow 列表 → 预取清单（QQ 行的 remote_cover 投影）。
pub fn targets_from_rows(rows: &[crate::library_view::SongRow]) -> Vec<(String, String)> {
    rows.iter()
        .filter_map(|row| row.remote_cover.clone().map(|url| (row.mid.clone(), url)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_view::SongRow;

    #[test]
    fn targets_from_rows_keeps_qq_remote_only() {
        let row = |mid: &str, remote: Option<&str>| SongRow {
            mid: mid.into(),
            source: 0,
            title: "t".into(),
            artists: String::new(),
            album: String::new(),
            duration_ms: 0,
            quality: String::new(),
            cover_uri: None,
            remote_cover: remote.map(String::from),
            folder: None,
        };
        let rows = vec![
            row("m1", Some("https://y.gtimg.cn/a.jpg")),
            row("m2", None),
            row("local:C:/x.mp3", None),
        ];
        let targets = targets_from_rows(&rows);
        assert_eq!(
            targets,
            vec![("m1".into(), "https://y.gtimg.cn/a.jpg".into())]
        );
    }

    /// 广播面：8 个曲目列表模型全部在列且无重复（防新增页面漏挂广播）。
    #[test]
    fn all_slots_are_distinct_and_complete() {
        let all = TrackSlot::all();
        assert_eq!(all.len(), 8);
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "slot 重复: {a:?} / {b:?}");
            }
        }
    }
}
