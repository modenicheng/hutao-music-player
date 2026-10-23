//! 在线内容页封面装载（M4 + D2/D7 泛化）：daemon CoverGet 换本地产物 +
//! cover_cache 统一缓存。
//!
//! 借鉴 `player_bridge::spawn_cover_fetch` 的去重/竞态防护，但目标是
//! `Data.discover-playlists` 等模型内的卡片（按 id 定位），非当前曲。
//! D7 抽出通用取图核心 [`fetch_cover`]（远端详情 hero / 榜单卡 / 搜索卡
//! 共用），去重账本进程级（失败出账允许重试）。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use slint::{Global, Model, Weak};

use crate::{AppWindow, Data};

/// 在账的 (kind, id, url)（在途或已成功；失败出账允许重试，进程级）。
/// 此前 thread_local + 跨线程 remove 实际删的是另一线程的账（无效操作），
/// 统一收敛为全局 Mutex，失败出账语义才真正成立。
static REQUESTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn requested_insert(key: &str) -> bool {
    let mut guard = REQUESTED.lock().expect("cover requests");
    guard
        .get_or_insert_with(HashSet::new)
        .insert(key.to_string())
}

fn requested_remove(key: &str) {
    let mut guard = REQUESTED.lock().expect("cover requests");
    if let Some(set) = guard.as_mut() {
        set.remove(key);
    }
}

/// 载入本地产物（cover_cache 256 桶：卡片封面消费方 ≤ 百px 级；不存在返回
/// None）。原线程本地 `HashMap<String, Image>` 无界缓存（内存治理工作③）
/// 已并入统一字节加权 LRU。
fn load_image(path: &str) -> Option<slint::Image> {
    crate::cover_cache::get_or_load(path, 256)
}

/// 通用取图核心（D-7）：CoverGet → 本地产物路径 → 回事件循环线程 256 桶
/// 装载并应用。去重键 `key` 由调用方组（kind|id|url）；失败出账允许重试。
/// `apply` 仅在取图与装载都成功时被调（UI 线程），竞态守卫（页参数/代数
/// 复核）由调用方闭包自带。
pub fn fetch_cover(
    ui_weak: Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
    key: String,
    url: String,
    apply: impl Fn(&AppWindow, slint::Image) + Send + 'static,
) {
    if !requested_insert(&key) {
        return; // 已请求过（在途或已成功）
    }
    runtime.spawn(async move {
        let Ok(hmp_core::Response::Cover(uri)) =
            crate::backend::request(hmp_core::Request::CoverGet { url }).await
        else {
            // 失败出账（2026-10-04 封面审计⑤）：一次失败不锁死整个会话，
            // 重进页面/重触发即可重试（触发源低频，无需退避）。
            requested_remove(&key);
            return;
        };
        let path = crate::covers::file_uri_to_path(&uri).unwrap_or(uri);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Some(image) = load_image(&path) {
                apply(&ui, image);
            } else {
                requested_remove(&key);
            }
        });
    });
}

/// 发现页歌单卡 mid ↔ 旁路表键的统一规范化（M1）：裸数字 id（历史登记
/// 形态）归一为 `qq:<id>`；已带前缀或非数字原样返回。登记
/// （`register_playlist_covers`）与匹配（`playlist_cover_url` 查表）两侧
/// **必须同走本函数**——`qq:` 前缀引入后卡片 mid 改形，两侧若不同源即
/// 封面静默失配（单测钉死）。
pub fn playlist_card_key(card_mid: &str) -> String {
    if card_mid.starts_with(crate::remote_details::REMOTE_PREFIX)
        || !card_mid.chars().all(|c| c.is_ascii_digit())
        || card_mid.is_empty()
    {
        card_mid.to_string()
    } else {
        format!("{}{}", crate::remote_details::REMOTE_PREFIX, card_mid)
    }
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
        // 占位封面无真实图；mid 与旁路表键同经 playlist_card_key 规范化（M1）
        let Some((id, url)) = playlist_cover_url(&card.mid) else {
            continue;
        };
        let key = format!("discover|{id}|{url}");
        if !requested_insert(&key) {
            continue; // 已请求过（在途或已成功）
        }
        let ui_weak = ui_weak.clone();
        let id = id.clone();
        runtime.spawn(async move {
            // 失败出账：冷启动窗口一次失败不再让卡片整个会话占位；
            // 重进 discover 页触发重试（触发源低频，无需退避）。
            let Ok(hmp_core::Response::Cover(uri)) =
                crate::backend::request(hmp_core::Request::CoverGet { url }).await
            else {
                requested_remove(&key);
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

/// 歌手页专辑网格卡换图（D-7：≤30 张，discover 模式）：按 mid 定位
/// `Data.artist-albums` 模型行原地换图，无匹配（页已切走/模型重建）即丢弃。
/// 远端卡 mid 带 `qq:` 前缀，entries 传裸 albummid——匹配前同前缀拼接。
pub fn refresh_artist_album_covers(
    ui_weak: Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
    entries: Vec<(String, String)>,
) {
    for (mid, url) in entries {
        let key = format!("artist-album|{mid}|{url}");
        let needle = format!("{}{mid}", crate::remote_details::REMOTE_PREFIX);
        fetch_cover(ui_weak.clone(), runtime, key, url, move |ui, image| {
            let data = Data::get(ui);
            let model = data.get_artist_albums();
            let Some(vec_model) = model
                .as_any()
                .downcast_ref::<slint::VecModel<crate::CoverCardData>>()
            else {
                return;
            };
            for i in 0..vec_model.iter().count() {
                let Some(mut card) = vec_model.row_data(i) else {
                    continue;
                };
                if card.mid.as_str() == needle {
                    card.cover = image.clone();
                    vec_model.set_row_data(i, card);
                    return;
                }
            }
        });
    }
}

/// 搜索结果卡换图（C6/D-7）：搜索响应与模型落地同线程同帧，URL 随调用
/// 直传（无 side-table）；mid 传裸 albummid/singermid，匹配前拼 `qq:`
/// 前缀（卡投影同式）。迟到图无匹配行（新搜索已重建模型）即丢弃。
pub fn refresh_search_covers(
    ui_weak: Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
    album_entries: Vec<(String, String)>,
    artist_entries: Vec<(String, String)>,
) {
    for (mid, url) in album_entries {
        let key = format!("search-album|{mid}|{url}");
        let needle = format!("{}{mid}", crate::remote_details::REMOTE_PREFIX);
        fetch_cover(ui_weak.clone(), runtime, key, url, move |ui, image| {
            swap_search_album_cover(ui, &needle, image);
        });
    }
    for (mid, url) in artist_entries {
        let key = format!("search-artist|{mid}|{url}");
        let needle = format!("{}{mid}", crate::remote_details::REMOTE_PREFIX);
        fetch_cover(ui_weak.clone(), runtime, key, url, move |ui, image| {
            swap_search_artist_cover(ui, &needle, image);
        });
    }
}

/// 搜索专辑卡换图（search-albums 模型按 mid 定位；无匹配即丢弃）。
fn swap_search_album_cover(ui: &AppWindow, mid: &str, image: slint::Image) {
    let model = Data::get(ui).get_search_albums();
    let Some(vec_model) = model
        .as_any()
        .downcast_ref::<slint::VecModel<crate::CoverCardData>>()
    else {
        return;
    };
    for i in 0..vec_model.iter().count() {
        let Some(mut card) = vec_model.row_data(i) else {
            continue;
        };
        if card.mid.as_str() == mid {
            card.cover = image;
            vec_model.set_row_data(i, card);
            return;
        }
    }
}

/// 搜索歌手卡换图（search-artists 模型按 mid 定位；无匹配即丢弃）。
fn swap_search_artist_cover(ui: &AppWindow, mid: &str, image: slint::Image) {
    let model = Data::get(ui).get_search_artists();
    let Some(vec_model) = model
        .as_any()
        .downcast_ref::<slint::VecModel<crate::ArtistCardData>>()
    else {
        return;
    };
    for i in 0..vec_model.iter().count() {
        let Some(mut card) = vec_model.row_data(i) else {
            continue;
        };
        if card.mid.as_str() == mid {
            card.cover = image;
            vec_model.set_row_data(i, card);
            return;
        }
    }
}

/// 歌单 id → 远程封面 URL（来自 discover 原始响应的旁路表）。
///
/// `CoverCardData` 不携带 URL（Slint 结构保持窄），Rust 侧以并发安全
/// 的 side-table 保存 id→url；`spawn_discover_load` 落模型时同步登记。
/// 键与卡片 mid 同经 [`playlist_card_key`] 规范化（M1）。
static PLAYLIST_COVERS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// 登记一批歌单封面 URL（discover 响应落地时调用；键规范化归一）。
pub fn register_playlist_covers(entries: Vec<(String, String)>) {
    let mut guard = PLAYLIST_COVERS.lock().expect("playlist covers");
    let map = guard.get_or_insert_with(HashMap::new);
    for (id, url) in entries {
        map.insert(playlist_card_key(&id), url);
    }
}

fn playlist_cover_url(card_mid: &str) -> Option<(String, String)> {
    let key = playlist_card_key(card_mid);
    let guard = PLAYLIST_COVERS.lock().expect("playlist covers");
    guard.as_ref()?.get(&key).map(|url| (key, url.clone()))
}

/// 读取 playlist 旁路表（单测用；键与登记侧同规范化）。
#[cfg(test)]
pub fn playlist_cover_of(card_mid: &str) -> Option<String> {
    let key = playlist_card_key(card_mid);
    let guard = PLAYLIST_COVERS.lock().expect("playlist covers");
    guard.as_ref()?.get(&key).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup_cover() {
        // 裸数字登记（历史形态）与 qq: 前缀查表同键命中（M1）
        register_playlist_covers(vec![("123456".into(), "https://y.gtimg.cn/x.jpg".into())]);
        assert_eq!(
            playlist_cover_of("123456").as_deref(),
            Some("https://y.gtimg.cn/x.jpg")
        );
        assert_eq!(
            playlist_cover_of("qq:123456").as_deref(),
            Some("https://y.gtimg.cn/x.jpg")
        );
        assert_eq!(playlist_cover_of("nope"), None);
    }

    /// M1 验收：推荐卡 mid（前缀形态）↔ 旁路表键一致——登记与匹配两侧
    /// 同走 playlist_card_key，任一侧改形都会在此失配。
    #[test]
    fn playlist_card_key_normalizes_both_directions() {
        // 裸数字 → 前缀；已前缀/非数字 → 原样
        assert_eq!(playlist_card_key("7468450811"), "qq:7468450811");
        assert_eq!(playlist_card_key("qq:7468450811"), "qq:7468450811");
        assert_eq!(playlist_card_key("sm-1"), "sm-1");
        assert_eq!(playlist_card_key(""), "");

        // 发现卡投影（bridge::discover_playlist_card 同式构造）与登记键一致
        let card_mid = playlist_card_key(&7468450811i64.to_string());
        register_playlist_covers(vec![(7468450811i64.to_string(), "u".into())]);
        assert_eq!(
            playlist_cover_url(&card_mid).map(|(k, _)| k),
            Some(card_mid.clone()),
            "卡片 mid 必须命中旁路表键"
        );
    }
}
