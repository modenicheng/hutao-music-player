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
    /// 远端专辑装载代数（同上）。
    static ALBUM_GENERATION: Cell<u64> = const { Cell::new(0) };
    /// 远端歌手装载代数（同上）。
    static ARTIST_GENERATION: Cell<u64> = const { Cell::new(0) };
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

// ————————————————————————————————————————————————
// 远端专辑详情（D2b）
// ————————————————————————————————————————————————

/// 远端专辑 meta（T5 验收口径）：年份（发行日期前 4，无值 "—"）/ 公司 /
/// 语种 / 流派 / 共 N 首（+ 超窗标注，D-4）。空值不占位。
fn remote_album_meta(page: &hmp_core::RemoteAlbumDetail) -> Vec<SharedString> {
    let mut items = vec![if page.time_public.len() >= 4 {
        page.time_public[..4].to_string()
    } else {
        "—".into()
    }];
    for text in [&page.company, &page.language, &page.genre] {
        if !text.is_empty() {
            items.push(text.clone());
        }
    }
    items.push(if page.has_more {
        format!("共 {} 首（展示前 {} 首）", page.total, page.tracks.len())
    } else {
        format!("共 {} 首", page.total)
    });
    items.into_iter().map(SharedString::from).collect()
}

/// 远端专辑装载入口（UI 线程被调，O1）。`param` 形如 `qq:<albummid>`。
pub fn spawn_remote_album_load(
    ui_weak: Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
    param: &str,
) {
    let Some((mid, generation)) = begin_album_load(&ui_weak, param) else {
        return;
    };
    let weak = ui_weak;
    let rt_apply = Arc::clone(runtime);
    runtime.spawn(async move {
        let result = crate::backend::request(hmp_core::Request::AlbumDetailGet { mid }).await;
        let _ = slint::invoke_from_event_loop(move || {
            apply_album_response(&weak, &rt_apply, generation, result);
        });
    });
}

/// 装载前半（纯 UI 线程部分）：置 state=1、校验参数、bump 代数。
/// 前缀后为空 → state=4 并返回 None。
pub fn begin_album_load(ui_weak: &Weak<AppWindow>, param: &str) -> Option<(String, u64)> {
    let ui = ui_weak.upgrade()?;
    let data = Data::get(&ui);
    data.set_album_state(1);
    let Some(mid) = remote_param(param).filter(|m| !m.is_empty()) else {
        data.set_album_state(4);
        return None;
    };
    Some((mid.to_string(), bump_album_generation()))
}

/// 当前远端专辑装载代数（迟到回包丢弃测试用）。
pub fn current_album_generation() -> u64 {
    ALBUM_GENERATION.with(|g| g.get())
}

fn bump_album_generation() -> u64 {
    ALBUM_GENERATION.with(|g| {
        let next = g.get().wrapping_add(1);
        g.set(next);
        next
    })
}

/// 回包落地（invoke 闭包体，UI 线程）：代数过期即弃；not-found（3）与
/// 失败（4）分态。歌手链接带 `qq:` 前缀（D-5，点击跳远端歌手页）。
pub fn apply_album_response(
    ui_weak: &Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
    generation: u64,
    result: Result<hmp_core::Response, crate::backend::BackendError>,
) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    if current_album_generation() != generation {
        return; // 迟到回包：页已切走/已重新装载，丢弃防覆盖
    }
    let data = Data::get(&ui);
    match result {
        Ok(hmp_core::Response::AlbumDetail(page)) => {
            data.set_album_name(page.name.as_str().into());
            data.set_album_artist(if page.singer_mid.is_empty() {
                SharedString::from("")
            } else {
                SharedString::from(format!("{REMOTE_PREFIX}{}", page.singer_mid))
            });
            data.set_album_meta_items(model_rc(remote_album_meta(&page)));
            // hero 封面：程序化占位先行，picurl 非空 → CoverGet 异步换真图
            data.set_album_cover(crate::covers::cover_image(&format!("album:{}", page.mid)));
            let rows: Vec<TrackRow> = page.tracks.iter().map(remote_track_row).collect();
            data.set_album_tracks(model_rc(rows));
            data.set_album_state(2);
            crate::track_covers::prefetch_tracks(
                runtime,
                page.tracks
                    .iter()
                    .filter(|s| !s.picurl.is_empty())
                    .map(|s| (s.mid.clone(), s.picurl.clone()))
                    .collect(),
            );
            if !page.picurl.is_empty() {
                let key = format!("album-hero|{}|{}", page.mid, page.picurl);
                crate::online_covers::fetch_cover(
                    ui_weak.clone(),
                    runtime,
                    key,
                    page.picurl.clone(),
                    move |ui, image| {
                        if current_album_generation() != generation {
                            return;
                        }
                        Data::get(ui).set_album_cover(image);
                    },
                );
            }
        }
        Ok(hmp_core::Response::Err { ref message, .. }) if is_not_found(message) => {
            data.set_album_state(3);
        }
        _ => {
            data.set_album_state(4);
        }
    }
}

// ————————————————————————————————————————————————
// 远端歌手详情（D2c，含歌手照片）
// ————————————————————————————————————————————————

/// 远端歌手 meta：歌曲/专辑总数（超窗标注首窗，D-4）；空域不占位。
fn remote_artist_meta(page: &hmp_core::RemoteArtistDetail) -> Vec<SharedString> {
    let songs = if (page.tracks.len() as i64) < page.song_total {
        format!(
            "{} 首歌曲（展示前 {} 首）",
            page.song_total,
            page.tracks.len()
        )
    } else {
        format!("{} 首歌曲", page.song_total)
    };
    let albums = if (page.albums.len() as i64) < page.album_total {
        format!(
            "{} 张专辑（展示前 {} 张）",
            page.album_total,
            page.albums.len()
        )
    } else {
        format!("{} 张专辑", page.album_total)
    };
    vec![songs.into(), albums.into()]
}

/// 歌手页专辑网格卡（RemoteAlbumBrief 投影）：mid 带 `qq:` 前缀
/// （点击进远端专辑详情，二级深入），副标题 "年份 · N 首"。
fn remote_artist_album_card(a: &hmp_core::RemoteAlbumBrief) -> crate::CoverCardData {
    crate::CoverCardData {
        mid: format!("{REMOTE_PREFIX}{}", a.mid).into(),
        title: a.name.as_str().into(),
        subtitle: format!("{} · {} 首", a.year_text, a.track_count).into(),
        cover: crate::covers::cover_image(&format!("album:{}", a.mid)),
    }
}

/// 远端歌手装载入口（UI 线程被调，O1）。`param` 形如 `qq:<singermid>`。
pub fn spawn_remote_artist_load(
    ui_weak: Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
    param: &str,
) {
    let Some((mid, generation)) = begin_artist_load(&ui_weak, param) else {
        return;
    };
    let weak = ui_weak;
    let rt_apply = Arc::clone(runtime);
    runtime.spawn(async move {
        let result = crate::backend::request(hmp_core::Request::ArtistDetailGet { mid }).await;
        let _ = slint::invoke_from_event_loop(move || {
            apply_artist_response(&weak, &rt_apply, generation, result);
        });
    });
}

/// 装载前半（纯 UI 线程部分）：置 state=1、校验参数、bump 代数。
/// 前缀后为空 → state=4 并返回 None。
pub fn begin_artist_load(ui_weak: &Weak<AppWindow>, param: &str) -> Option<(String, u64)> {
    let ui = ui_weak.upgrade()?;
    let data = Data::get(&ui);
    data.set_artist_state(1);
    let Some(mid) = remote_param(param).filter(|m| !m.is_empty()) else {
        data.set_artist_state(4);
        return None;
    };
    Some((mid.to_string(), bump_artist_generation()))
}

/// 当前远端歌手装载代数（迟到回包丢弃测试用）。
pub fn current_artist_generation() -> u64 {
    ARTIST_GENERATION.with(|g| g.get())
}

fn bump_artist_generation() -> u64 {
    ARTIST_GENERATION.with(|g| {
        let next = g.get().wrapping_add(1);
        g.set(next);
        next
    })
}

/// 回包落地（invoke 闭包体，UI 线程）：代数过期即弃；not-found（3）与
/// 失败（4）分态。歌手照片 = 主页头部 singer_pic（§20/§23 边界闭合的
/// 数据源）；域外/空 → 程序化占位不换图。
pub fn apply_artist_response(
    ui_weak: &Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
    generation: u64,
    result: Result<hmp_core::Response, crate::backend::BackendError>,
) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    if current_artist_generation() != generation {
        return; // 迟到回包：页已切走/已重新装载，丢弃防覆盖
    }
    let data = Data::get(&ui);
    match result {
        Ok(hmp_core::Response::ArtistDetail(page)) => {
            data.set_artist_name(page.name.as_str().into());
            data.set_artist_meta_items(model_rc(remote_artist_meta(&page)));
            // 照片：程序化占位先行，photo_url 非空 → CoverGet 换真图
            data.set_artist_photo(crate::covers::cover_image(&format!("artist:{}", page.mid)));
            let rows: Vec<TrackRow> = page.tracks.iter().map(remote_track_row).collect();
            data.set_artist_tracks(model_rc(rows));
            let cards: Vec<crate::CoverCardData> =
                page.albums.iter().map(remote_artist_album_card).collect();
            data.set_artist_albums(model_rc(cards));
            data.set_artist_state(2);
            crate::track_covers::prefetch_tracks(
                runtime,
                page.tracks
                    .iter()
                    .filter(|s| !s.picurl.is_empty())
                    .map(|s| (s.mid.clone(), s.picurl.clone()))
                    .collect(),
            );
            if !page.photo_url.is_empty() {
                let key = format!("artist-photo|{}|{}", page.mid, page.photo_url);
                crate::online_covers::fetch_cover(
                    ui_weak.clone(),
                    runtime,
                    key,
                    page.photo_url.clone(),
                    move |ui, image| {
                        if current_artist_generation() != generation {
                            return;
                        }
                        Data::get(ui).set_artist_photo(image);
                    },
                );
            }
            // 专辑网格卡换图（≤30 张，discover 模式：模型已落地，按 mid
            // 原地换图；离页无匹配即丢弃）
            crate::online_covers::refresh_artist_album_covers(
                ui_weak.clone(),
                runtime,
                page.albums
                    .iter()
                    .filter(|a| !a.picurl.is_empty())
                    .map(|a| (a.mid.clone(), a.picurl.clone()))
                    .collect(),
            );
        }
        Ok(hmp_core::Response::Err { ref message, .. }) if is_not_found(message) => {
            data.set_artist_state(3);
        }
        _ => {
            data.set_artist_state(4);
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
    fn remote_album_meta_year_company_language_and_window() {
        let mut page = hmp_core::RemoteAlbumDetail {
            mid: "am-1".into(),
            name: "十一月的萧邦".into(),
            singer: "周杰伦".into(),
            singer_mid: "sm-1".into(),
            company: "杰威尔音乐".into(),
            genre: "流行".into(),
            language: "国语".into(),
            desc: String::new(),
            time_public: "2005-11-01".into(),
            picurl: String::new(),
            tracks: vec![hmp_core::DiscoverNewSong::default(); 12],
            total: 12,
            has_more: false,
        };
        let meta = remote_album_meta(&page);
        assert_eq!(meta[0].as_str(), "2005");
        assert_eq!(meta[1].as_str(), "杰威尔音乐");
        assert_eq!(meta[2].as_str(), "国语");
        assert_eq!(meta[3].as_str(), "流行");
        assert_eq!(meta[4].as_str(), "共 12 首");

        // 超窗标注 + 缺发行日期/公司 → "—" 与跳过空值
        page.has_more = true;
        page.total = 130;
        page.time_public = String::new();
        page.company = String::new();
        let meta = remote_album_meta(&page);
        assert_eq!(meta[0].as_str(), "—");
        assert_eq!(meta[1].as_str(), "国语");
        assert_eq!(meta.last().unwrap().as_str(), "共 130 首（展示前 12 首）");
    }

    #[test]
    fn remote_artist_meta_and_album_card_projection() {
        let page = hmp_core::RemoteArtistDetail {
            mid: "sm-1".into(),
            name: "周杰伦".into(),
            photo_url: "https://y.gtimg.cn/p.jpg".into(),
            song_total: 1012,
            album_total: 139,
            tracks: vec![hmp_core::DiscoverNewSong::default(); 50],
            albums: vec![hmp_core::RemoteAlbumBrief {
                mid: "a-1".into(),
                name: "范特西".into(),
                picurl: String::new(),
                year_text: "2001".into(),
                track_count: 10,
            }],
            desc: String::new(),
        };
        let meta = remote_artist_meta(&page);
        assert_eq!(meta[0].as_str(), "1012 首歌曲（展示前 50 首）");
        assert_eq!(meta[1].as_str(), "139 张专辑（展示前 1 张）");

        // 专辑网格卡：qq: 前缀导航 + 年份·曲数副标题（T6 验收）
        let card = remote_artist_album_card(&page.albums[0]);
        assert_eq!(card.mid.as_str(), "qq:a-1");
        assert_eq!(card.title.as_str(), "范特西");
        assert_eq!(card.subtitle.as_str(), "2001 · 10 首");

        // 首窗即全集 → 无标注
        let exact = hmp_core::RemoteArtistDetail {
            song_total: 50,
            album_total: 1,
            ..page
        };
        let meta = remote_artist_meta(&exact);
        assert_eq!(meta[0].as_str(), "50 首歌曲");
        assert_eq!(meta[1].as_str(), "1 张专辑");
    }

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
