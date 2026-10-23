//! 远端详情装载（D2 内容动线）：`qq:` 前缀导航参数 → daemon 只读详情接口。
//!
//! 装载形态对齐 `spawn_discover_load` 先例，差异在加载态置位落点：本模块
//! 入口在 UI 线程被调（on_navigate / `bridge::apply_route` 栈内），state=1
//! **直接 set**（O1——勿套 invoke_from_event_loop，TestingBackend 下会挂起
//! mock 泵，bridge.rs `spawn_discover_load` 注释在案）；异步回包再经
//! invoke 回事件循环线程落模型。每页一个代数计数器（对齐
//! `SEARCH_GENERATION` 先例）：快速导航 A→B 时 A 的迟到回包按代数丢弃
//! （R4），防旧数据覆盖新页。
//!
//! 本地投影路径不受影响：本地参数（歌单=DB id、专辑/歌手=展示名）不含
//! `qq:` 前缀，`bridge::load_detail` 原样处理；远端参数在本地 miss 后才
//! 分流到本模块（本地事实优先，R3）。库变更重放（schedule_refresh）对
//! 远端参数返回 `DetailData::None` no-op，远端页面数据不被误清（D-1）。

use std::cell::Cell;
use std::sync::Arc;

use slint::{Global, SharedString, Weak};

use crate::{AppWindow, Data, TrackRow};

/// 远端导航参数前缀（D-1）：`qq:` 后接远端实体 id（歌单 disstid / 专辑
/// albummid / 歌手 singermid），与本地参数空间（数字 id / 展示名）不冲突。
pub const REMOTE_PREFIX: &str = "qq:";

thread_local! {
    /// 远端歌单装载代数（读写都在事件循环线程，单线程计数即可；
    /// 导航进入远端歌单页即 bump，回包落地前比对，过期即弃）。
    static PLAYLIST_GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// 剥离 `qq:` 前缀（无前缀 → None；本地/远端分流判据，单测锚定）。
pub fn remote_param(param: &str) -> Option<&str> {
    param.strip_prefix(REMOTE_PREFIX)
}

/// 远端详情曲目行（`DiscoverNewSong` 投影）：行内歌手/专辑链接带 `qq:`
/// 前缀（D-5）——TrackTable 既有链接零改动即可跳远端详情；空 mid → 空串
/// （链接禁用，与服务端无值口径一致）。
pub fn remote_track_row(s: &hmp_core::DiscoverNewSong) -> TrackRow {
    fn linked(mid: &str) -> SharedString {
        if mid.is_empty() {
            SharedString::from("")
        } else {
            SharedString::from(format!("{REMOTE_PREFIX}{mid}"))
        }
    }
    TrackRow {
        mid: s.mid.as_str().into(),
        source: 0,
        title: s.name.as_str().into(),
        artists: s.singer.as_str().into(),
        artist_mid: linked(&s.artist_mid),
        album: s.album.as_str().into(),
        album_mid: linked(&s.album_mid),
        duration_ms: (s.interval * 1000) as i32,
        quality: "".into(),
    }
}

/// 远端歌单 meta 文案（D-4 首窗诚实标注）：共 N 首（+ 超窗标注）/ 创建者
/// / 播放数。空值不占位（不伪造）。
fn remote_playlist_meta(page: &hmp_core::RemotePlaylistDetail) -> Vec<SharedString> {
    let mut items = vec![if page.has_more {
        format!("共 {} 首（展示前 {} 首）", page.total, page.tracks.len())
    } else {
        format!("共 {} 首", page.total)
    }];
    if !page.creator.is_empty() {
        items.push(format!("创建者 {}", page.creator));
    }
    if page.listen_num > 0 {
        items.push(format!("{} 次播放", page.listen_num));
    }
    items.into_iter().map(SharedString::from).collect()
}

/// daemon 错误文案 → 未找到判定（content.rs "playlist/album/artist not
/// found (code …)"；区别于离线/网络失败，分态 3/4）。
fn is_not_found(message: &str) -> bool {
    message.contains("not found")
}

/// 远端歌单装载入口（UI 线程被调，O1）：置加载态 + 代数 bump + 异步拉取。
/// `param` 形如 `qq:<disstid>`（`bridge::apply_route` 分流而来）。
pub fn spawn_remote_playlist_load(
    ui_weak: Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
    param: &str,
) {
    let Some((id, generation)) = begin_playlist_load(&ui_weak, param) else {
        return;
    };
    let weak = ui_weak;
    let rt = Arc::clone(runtime);
    let rt_apply = Arc::clone(runtime);
    rt.spawn(async move {
        let result = crate::backend::request(hmp_core::Request::PlaylistDetailGet { id }).await;
        let _ = slint::invoke_from_event_loop(move || {
            apply_playlist_response(&weak, &rt_apply, generation, result);
        });
    });
}

/// 装载前半（纯 UI 线程部分；集成测试直接驱动，不触网）：置 state=1、
/// 校验参数、bump 代数。无效参数（前缀后非数字）→ state=4 并返回 None。
pub fn begin_playlist_load(ui_weak: &Weak<AppWindow>, param: &str) -> Option<(i64, u64)> {
    let ui = ui_weak.upgrade()?;
    let data = Data::get(&ui);
    data.set_playlist_state(1);
    let Some(id) = remote_param(param).and_then(|mid| mid.parse::<i64>().ok()) else {
        data.set_playlist_state(4);
        return None;
    };
    Some((id, bump_playlist_generation()))
}

/// 当前远端歌单装载代数（迟到回包丢弃测试用）。
pub fn current_playlist_generation() -> u64 {
    PLAYLIST_GENERATION.with(|g| g.get())
}

fn bump_playlist_generation() -> u64 {
    PLAYLIST_GENERATION.with(|g| {
        let next = g.get().wrapping_add(1);
        g.set(next);
        next
    })
}

/// 回包落地（invoke 闭包体，UI 线程）：代数过期即弃（R4）；未找到（3，
/// 服务端 not-found）与失败（4，离线/网络错误）分态。`*-found` 由本地
/// 路径同步维护（M2），远端路径只写 state。
pub fn apply_playlist_response(
    ui_weak: &Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
    generation: u64,
    result: Result<hmp_core::Response, crate::backend::BackendError>,
) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    if current_playlist_generation() != generation {
        return; // 迟到回包：页已切走/已重新装载，丢弃防覆盖
    }
    let data = Data::get(&ui);
    match result {
        Ok(hmp_core::Response::PlaylistDetail(page)) => {
            data.set_playlist_name(page.name.as_str().into());
            data.set_playlist_meta_items(model_rc(remote_playlist_meta(&page)));
            // hero 封面：程序化占位先行，picurl 非空 → CoverGet 异步换真图
            data.set_playlist_cover(crate::covers::cover_image(&format!("playlist:{}", page.id)));
            let rows: Vec<TrackRow> = page.tracks.iter().map(remote_track_row).collect();
            data.set_playlist_tracks(model_rc(rows));
            data.set_playlist_state(2);
            // 行封面预取（picurl 已是完整地址；daemon 盘缓存命中零出网）
            crate::track_covers::prefetch_tracks(
                runtime,
                page.tracks
                    .iter()
                    .filter(|s| !s.picurl.is_empty())
                    .map(|s| (s.mid.clone(), s.picurl.clone()))
                    .collect(),
            );
            if !page.picurl.is_empty() {
                // hero 换图（256 桶，176px 消费）；应用前经代数守卫复核
                // 页参数未变，迟到图不覆盖新页
                let key = format!("playlist-hero|{}|{}", page.id, page.picurl);
                crate::online_covers::fetch_cover(
                    ui_weak.clone(),
                    runtime,
                    key,
                    page.picurl.clone(),
                    move |ui, image| {
                        if current_playlist_generation() != generation {
                            return;
                        }
                        Data::get(ui).set_playlist_cover(image);
                    },
                );
            }
        }
        Ok(hmp_core::Response::Err { ref message, .. }) if is_not_found(message) => {
            data.set_playlist_state(3);
        }
        // 其余（协议错误 / 离线 / 网络失败 / 应答类型不符）→ 失败态
        _ => {
            data.set_playlist_state(4);
        }
    }
}

fn model_rc<T: 'static + Clone>(items: Vec<T>) -> slint::ModelRc<T> {
    slint::ModelRc::new(slint::VecModel::from(items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_param_strips_prefix_only() {
        assert_eq!(remote_param("qq:8522132521"), Some("8522132521"));
        assert_eq!(remote_param("qq:0025NhlN2yWrP4"), Some("0025NhlN2yWrP4"));
        // 本地参数空间：数字 id / 展示名 / 空——一律不分流
        assert_eq!(remote_param("123"), None);
        assert_eq!(remote_param("十一月的萧邦"), None);
        assert_eq!(remote_param(""), None);
        // 前缀重复/近形不误判
        assert_eq!(remote_param("qqqq:1"), None);
    }

    #[test]
    fn remote_track_row_prefixes_links_and_disables_empty() {
        let song = hmp_core::DiscoverNewSong {
            mid: "m-1".into(),
            name: "夜曲".into(),
            singer: "周杰伦".into(),
            album: "十一月的萧邦".into(),
            interval: 226,
            picurl: String::new(),
            album_mid: "am-1".into(),
            artist_mid: "sm-1".into(),
        };
        let row = remote_track_row(&song);
        assert_eq!(row.artist_mid.as_str(), "qq:sm-1");
        assert_eq!(row.album_mid.as_str(), "qq:am-1");
        assert_eq!(row.duration_ms, 226_000);
        assert_eq!(row.source, 0);

        // 空 mid → 链接禁用（空串），与 TrackTable 判空口径一致
        let bare = hmp_core::DiscoverNewSong {
            album_mid: String::new(),
            artist_mid: String::new(),
            ..song
        };
        let row = remote_track_row(&bare);
        assert_eq!(row.artist_mid.as_str(), "");
        assert_eq!(row.album_mid.as_str(), "");
    }

    #[test]
    fn remote_playlist_meta_marks_window_and_optional_fields() {
        let mut page = hmp_core::RemotePlaylistDetail {
            id: 1,
            name: "华语经典".into(),
            creator: "胡桃".into(),
            picurl: String::new(),
            listen_num: 12345,
            tracks: vec![hmp_core::DiscoverNewSong::default(); 100],
            total: 268,
            has_more: true,
        };
        let meta = remote_playlist_meta(&page);
        assert_eq!(meta[0].as_str(), "共 268 首（展示前 100 首）");
        assert_eq!(meta[1].as_str(), "创建者 胡桃");
        assert_eq!(meta[2].as_str(), "12345 次播放");

        // 无超窗/无创建者/零播放 → 单条诚实文案（total 与窗口一致）
        page.has_more = false;
        page.total = page.tracks.len() as i64;
        page.creator = String::new();
        page.listen_num = 0;
        let meta = remote_playlist_meta(&page);
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].as_str(), "共 100 首");
    }
}
