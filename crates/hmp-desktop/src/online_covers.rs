//! 在线内容页封面装载（M4）：daemon CoverGet 换本地产物 + 线程本地 Image 缓存。
//!
//! 借鉴 `player_bridge::spawn_cover_fetch` 的去重/竞态防护，但目标是
//! `Data.discover-playlists` 模型内的卡片（按歌单 id 定位），非当前曲。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use slint::{Global, Image, Model, Weak};

use crate::{AppWindow, Data};

thread_local! {
    /// 已发起过的 (kind, id, url) 三元组；同图不重复出网。
    static REQUESTED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// 本地产物路径 → Image（与 player_bridge 的 load_cover_cached 同策略）。
    static IMAGE_CACHE: RefCell<HashMap<String, Image>> = RefCell::new(HashMap::new());
}

/// 载入本地产物（路径缓存；不存在返回 None）。
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

/// 把 `Data.discover-playlists` 中封面对应的模型项替换为真实图片。
///
/// `kind` 用于区分来源（"discover" / "guess"），与 id、url 组成去重键。
/// 定位方式：全模型线性扫描同 id 项（30 项规模可忽略）；无 id 匹配则丢弃
/// （用户已离开页面/刷新重建模型——迟到图不重试，刷新会再次触发）。
pub fn refresh_discover_covers(
    ui_weak: Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
) {
    let Some(ui) = ui_weak.upgrade() else { return };
    let data = Data::get(&ui);
    let model = data.get_discover_playlists();
    let Some(vec_model) = model
        .as_any()
        .downcast_ref::<slint::VecModel<crate::CoverCardData>>()
    else {
        return;
    };
    for i in 0..vec_model.iter().count() {
        let card = vec_model.row_data(i).unwrap_or_default();
        // 占位封面无真实图；mid 即歌单 id
        let Some((id, url)) = playlist_cover_url(&ui, &card.mid) else {
            continue;
        };
        let key = format!("discover|{id}|{url}");
        if REQUESTED.with(|set| !set.borrow_mut().insert(key)) {
            continue; // 已请求过（完成或失败均不重发）
        }
        let ui_weak = ui_weak.clone();
        let id = id.clone();
        runtime.spawn(async move {
            let Ok(hmp_core::Response::Cover(uri)) =
                crate::backend::request(hmp_core::Request::CoverGet { url }).await
            else {
                return;
            };
            let path = crate::covers::file_uri_to_path(&uri).unwrap_or(uri);
            let ui_weak2 = ui_weak.clone();
            let id2 = id.clone();
            let _ = slint::invoke_from_event_loop(move || {
                apply_playlist_cover(ui_weak2, &id2, &path);
            });
        });
    }
}

/// 应用到模型：找到同 id 卡片则更新封面（页面已切走则忽略）。
fn apply_playlist_cover(ui_weak: Weak<AppWindow>, id: &str, path: &str) {
    let Some(image) = load_image(path) else {
        return;
    };
    let Some(ui) = ui_weak.upgrade() else { return };
    let data = Data::get(&ui);
    let model = data.get_discover_playlists();
    let Some(vec_model) = model
        .as_any()
        .downcast_ref::<slint::VecModel<crate::CoverCardData>>()
    else {
        return;
    };
    for i in 0..vec_model.iter().count() {
        let mut card = match vec_model.row_data(i) {
            Some(c) => c,
            None => continue,
        };
        if card.mid.as_str() == id {
            card.cover = image.clone();
            vec_model.set_row_data(i, card);
            return;
        }
    }
}

/// 歌单 id → 远程封面 URL（来自 discover 原始响应的旁路表）。
///
/// `CoverCardData` 不携带 URL（Slint 结构保持窄），Rust 侧以并发安全
/// 的 side-table 保存 id→url；`spawn_discover_load` 落模型时同步登记。
use std::sync::Mutex;
static PLAYLIST_COVERS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// 登记一批歌单封面 URL（discover 响应落地时调用）。
pub fn register_playlist_covers(entries: Vec<(String, String)>) {
    let mut guard = PLAYLIST_COVERS.lock().expect("playlist covers");
    let map = guard.get_or_insert_with(HashMap::new);
    for (id, url) in entries {
        map.insert(id, url);
    }
}

fn playlist_cover_url(_ui: &AppWindow, id: &str) -> Option<(String, String)> {
    let guard = PLAYLIST_COVERS.lock().expect("playlist covers");
    guard
        .as_ref()?
        .get(id)
        .map(|url| (id.to_string(), url.clone()))
}

/// 读取 playlist 旁路表（单测用）。
#[cfg(test)]
pub fn playlist_cover_of(id: &str) -> Option<String> {
    let guard = PLAYLIST_COVERS.lock().expect("playlist covers");
    guard.as_ref()?.get(id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup_cover() {
        register_playlist_covers(vec![("123456".into(), "https://y.gtimg.cn/x.jpg".into())]);
        assert_eq!(
            playlist_cover_of("123456").as_deref(),
            Some("https://y.gtimg.cn/x.jpg")
        );
        assert_eq!(playlist_cover_of("nope"), None);
    }
}
