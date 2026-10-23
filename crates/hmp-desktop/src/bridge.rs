//! UI 桥接：媒体库快照装载为 Slint 模型 + 导航/主题/音质/过滤回调绑定。
//! （旧版对接 AppCore 的桥随旧 UI 契约废弃，M8 数据接线时重写回来；
//! 页面数据源 = [`library_view`] 直读 library.sqlite3，播放/命令桥另行接线。）

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::{ComponentHandle, Global, ModelRc, SharedString, Weak};

use crate::covers::cover_image;
use crate::format::{format_bytes, format_long_duration};
use crate::library_view::{self, PlaylistEntry, SongRow, local_cover_image};
use crate::prefs::Prefs;
use crate::{
    AppWindow, CoverCardData, Data, FolderRow, Nav, PlaylistCover, Quality, Theme, TopCardData,
    TopGroupData, TrackRow,
};

const NAV_HISTORY_CAP: usize = 50;
/// 库变更刷新防抖：watcher 批处理每 ~1s bump 一次，扫描期连发——
/// 静默 500ms 才触发一次重查，避免 UI 随扫描进度反复重载。
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(500);

// 搜索请求代计数（app.rs NetworkRequestState 的轻量同思路）：上一请求未
// 返回时再次搜索，两个响应到达顺序不保证——落结果前比对代次，过期即弃，
// 防旧关键词结果覆盖新关键词。回调与结果落地都经事件循环线程（on_search
// 本体 / invoke_from_event_loop），单线程计数即可，无需原子量。
thread_local! {
    static SEARCH_GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// 扫码登录轮询代次（重新发起/取消即 bump；旧轮询循环的迟到响应按代次丢弃）。
/// 轮询循环活在 runtime 工作线程，不能像 SEARCH_GENERATION 那样用
/// thread_local（其正确性依赖「读写都在事件循环线程」），统一 AtomicU64。
static LOGIN_GENERATION: AtomicU64 = AtomicU64::new(0);
/// 登录轮询间隔（`LoginQrPoll`；与 CLI PollInterval default 同量级）。
const LOGIN_POLL_INTERVAL: Duration = Duration::from_millis(1500);

fn model<T: 'static + Clone>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

use slint::VecModel;

/// 歌单卡（副标题来自媒体库投影的 "N 首"——媒体库无播放计数，不伪造"X次播放"；
/// 封面：QQ 歌单封面本地产物优先，缺失回退中性占位）。
fn playlist_card(entry: &PlaylistEntry) -> CoverCardData {
    CoverCardData {
        mid: entry.id.clone().into(),
        title: entry.name.clone().into(),
        subtitle: entry.subtitle.clone().into(),
        cover: local_cover_image(entry.cover_uri.as_deref())
            .unwrap_or_else(|| cover_image(&format!("playlist:{}", entry.id))),
    }
}

/// 侧栏歌单封面：QQ 歌单封面本地产物优先（与库页歌单卡同源），
/// 缺失回退中性占位（playlist:{id} seed，covers 程序化生成）。
fn sidebar_cover(entry: &PlaylistEntry) -> PlaylistCover {
    PlaylistCover {
        // id 原样透传：与库页歌单卡（playlist_card → CoverCardData.mid）同一
        // 字符串形式，歌单详情页按它 parse 成 i64 查库
        id: entry.id.clone().into(),
        name: entry.name.clone().into(),
        // 归属随行透传：右键"加入播放列表"按曲源兼容性预禁用（owned 拒本地曲）
        relation: entry.relation.clone().into(),
        image: local_cover_image(entry.cover_uri.as_deref())
            .unwrap_or_else(|| cover_image(&format!("playlist:{}", entry.id))),
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

/// 搜索结果行（smartbox 窄投影：无专辑/时长 → 对应列收起）。表格行不带
/// 封面（TrackRow 已剥 `cover`，队列行走 QueueRow）。
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
    }
}

/// 发现页/榜单/猜你喜欢共用曲目行（DiscoverNewSong 投影；有专辑与时长）。
fn discover_track_row(s: &hmp_core::DiscoverNewSong) -> TrackRow {
    TrackRow {
        mid: s.mid.as_str().into(),
        source: 0,
        title: s.name.as_str().into(),
        artists: s.singer.as_str().into(),
        artist_mid: "".into(),
        album: s.album.as_str().into(),
        album_mid: "".into(),
        duration_ms: (s.interval * 1000) as i32,
        quality: "".into(),
    }
}

/// 搜索专辑卡（SearchAlbum → CoverCardData；C6）。mid 带 `qq:` 前缀——
/// 点击直达远端专辑详情（D-1）；副标题 = 歌手名。
fn search_album_card(a: &hmp_core::SearchAlbum) -> CoverCardData {
    CoverCardData {
        mid: format!("{}{}", crate::remote_details::REMOTE_PREFIX, a.mid).into(),
        title: a.name.as_str().into(),
        subtitle: a.singer.as_str().into(),
        cover: cover_image(&format!("album:{}", a.mid)),
    }
}

/// 搜索歌手卡（SearchSinger → ArtistCardData；C6）。mid 带 `qq:` 前缀；
/// 占位 seed artist:{mid} 与歌手页 hero 同源同图。
fn search_singer_card(s: &hmp_core::SearchSinger) -> crate::ArtistCardData {
    crate::ArtistCardData {
        mid: format!("{}{}", crate::remote_details::REMOTE_PREFIX, s.mid).into(),
        name: s.name.as_str().into(),
        cover: cover_image(&format!("artist:{}", s.mid)),
    }
}

/// 搜索有结果判定（C6）：歌曲/专辑/歌手**三组全空**才算无结果（state=3）。
fn search_has_results(
    rows: &[TrackRow],
    albums: &[CoverCardData],
    artists: &[crate::ArtistCardData],
) -> bool {
    !(rows.is_empty() && albums.is_empty() && artists.is_empty())
}

/// 发现页歌单卡片（DiscoverPlaylist → CoverCardData；封面经 covers 程序化占位，
/// 真实封面 URL 经 CoverGet 异步补齐——online_covers 旁路表 + 换图）。
/// mid 带 `qq:` 前缀（D-1）：点击即走远端歌单详情装载（本地 DB id 参数
/// 空间不含前缀，互不污染）；与封面旁路表键同经 `playlist_card_key`
/// 规范化（M1，两侧同帧同源）。
fn discover_playlist_card(p: &hmp_core::DiscoverPlaylist) -> CoverCardData {
    let subtitle = if p.creator.is_empty() {
        format!("{} 首", p.songnum)
    } else {
        format!("{} 首 · {}", p.songnum, p.creator)
    };
    CoverCardData {
        mid: crate::online_covers::playlist_card_key(&p.id.to_string()).into(),
        title: p.title.as_str().into(),
        subtitle: subtitle.into(),
        cover: crate::covers::cover_image(&format!("playlist:{}", p.id)),
    }
}

/// 拉取发现页（免登录）；失败/离线 → 专属失败态（同搜索页码位约定）。
fn spawn_discover_load(ui_weak: Weak<AppWindow>, runtime: &Arc<crate::backend::BackendRuntime>) {
    let weak = ui_weak;
    let rt: Arc<crate::backend::BackendRuntime> = Arc::clone(runtime);
    let rt_spawn = Arc::clone(&rt);
    rt_spawn.spawn(async move {
        // 置加载态也挪到 tokio 线程：事件循环线程内（如 on_navigate 栈）直接
        // invoke_from_event_loop 在 TestingBackend 下会挂起整个 mock 泵。
        let w0 = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w0.upgrade() {
                Data::get(&ui).set_discover_state(1);
            }
        });
        let result = crate::backend::request(hmp_core::Request::DiscoverGet {
            songlist_page: 1,
            new_song_type: 5,
        })
        .await;
        let rt2 = Arc::clone(&rt);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let data = Data::get(&ui);
            let runtime = Arc::clone(&rt2);
            match result {
                Ok(hmp_core::Response::Discover(page)) => {
                    let cards: Vec<CoverCardData> =
                        page.playlists.iter().map(discover_playlist_card).collect();
                    let tracks: Vec<TrackRow> =
                        page.new_songs.iter().map(discover_track_row).collect();
                    let has_more = page.has_more_playlists;
                    let empty = cards.is_empty() && tracks.is_empty();
                    // 登记歌单封面 URL 旁路表并触发异步取图（daemon CoverGet）
                    crate::online_covers::register_playlist_covers(
                        page.playlists
                            .iter()
                            .filter(|p| !p.picurl.is_empty())
                            .map(|p| (p.id.to_string(), p.picurl.clone()))
                            .collect(),
                    );
                    data.set_discover_playlists(model(cards));
                    data.set_discover_new_songs(model(tracks));
                    data.set_discover_has_more(has_more);
                    data.set_discover_state(if empty { 3 } else { 2 });
                    // 模型落地后补真图（占位渐变 → 真实封面）
                    crate::online_covers::refresh_discover_covers(ui.as_weak(), &runtime);
                    // 新歌行封面预取（DiscoverNewSong.picurl 是套好模板的完整地址）
                    crate::track_covers::prefetch_tracks(
                        &runtime,
                        page.new_songs
                            .iter()
                            .filter(|s| !s.picurl.is_empty())
                            .map(|s| (s.mid.clone(), s.picurl.clone()))
                            .collect(),
                    );
                }
                _ => {
                    data.set_discover_state(4);
                }
            }
        });
    });
}

/// 拉取排行榜详情（免登录；Nav.param 为 top id 字符串）。
fn spawn_top_detail_load(
    ui_weak: Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
    param: SharedString,
) {
    let Ok(top_id) = param.as_str().parse::<i64>() else {
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                Data::get(&ui).set_top_detail_state(4);
            }
        });
        return;
    };
    let weak = ui_weak;
    let rt = Arc::clone(runtime);
    runtime.spawn(async move {
        // 置加载态在 tokio 线程（同 discover：事件循环栈内 invoke 会挂 TestingBackend 泵）
        let w0 = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w0.upgrade() {
                let data = Data::get(&ui);
                data.set_top_detail_state(1);
                // 换榜加载起点清残留：found=false 让页头退回占位文案，
                // 不再显示上一榜的名字/条数
                data.set_top_detail_found(false);
            }
        });
        let result = crate::backend::request(hmp_core::Request::TopDetailGet {
            top_id,
            num: 100,
            page: 1,
        })
        .await;
        let rt2 = Arc::clone(&rt);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let data = Data::get(&ui);
            match result {
                Ok(hmp_core::Response::TopDetail(page)) if !page.songs.is_empty() => {
                    let tracks: Vec<TrackRow> = page
                        .songs
                        .iter()
                        .map(|s| {
                            let p = hmp_core::DiscoverNewSong {
                                mid: s.mid.clone(),
                                name: s.name.clone(),
                                singer: s.singer.clone(),
                                album: s.album.clone(),
                                interval: s.interval,
                                picurl: s.picurl.clone(),
                                // TopSongDto 不携带 mid（D-5 只覆盖远端详情行）
                                // → 链接禁用，与既有行为一致。
                                album_mid: String::new(),
                                artist_mid: String::new(),
                            };
                            discover_track_row(&p)
                        })
                        .collect();
                    let name = page.name.as_str().into();
                    let update = page.update_time.as_str().into();
                    // meta 条数与实际装载行数一致：请求固定 num=100，daemon
                    // total 是全榜总数（如热歌榜 300），表格并没有那么多行
                    let total = tracks.len() as i32;
                    data.set_top_detail_tracks(model(tracks));
                    data.set_top_detail_name(name);
                    data.set_top_detail_update(update);
                    data.set_top_detail_total(total);
                    data.set_top_detail_found(true);
                    data.set_top_detail_state(2);
                    // 榜单行封面预取（同 discover 新歌行：picurl 已是完整地址）
                    crate::track_covers::prefetch_tracks(
                        &rt2,
                        page.songs
                            .iter()
                            .filter(|s| !s.picurl.is_empty())
                            .map(|s| (s.mid.clone(), s.picurl.clone()))
                            .collect(),
                    );
                }
                Ok(hmp_core::Response::TopDetail(_)) => {
                    data.set_top_detail_state(3);
                }
                _ => {
                    data.set_top_detail_state(4);
                }
            }
        });
    });
}

/// 拉取排行榜分类（免登录）。
fn spawn_top_category_load(
    ui_weak: Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
) {
    let weak = ui_weak;
    let rt = Arc::clone(runtime);
    let rt_spawn = Arc::clone(&rt);
    rt_spawn.spawn(async move {
        // 置加载态在 tokio 线程（同 discover）
        let w0 = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w0.upgrade() {
                Data::get(&ui).set_top_state(1);
            }
        });
        let result = crate::backend::request(hmp_core::Request::TopCategoryGet).await;
        let rt2 = Arc::clone(&rt);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let data = Data::get(&ui);
            match result {
                Ok(hmp_core::Response::TopCategory(page)) if !page.groups.is_empty() => {
                    let groups: Vec<TopGroupData> = page
                        .groups
                        .iter()
                        .map(|g| TopGroupData {
                            id: g.id.to_string().into(),
                            name: g.name.as_str().into(),
                            tops: model(g.tops.iter().map(top_card).collect::<Vec<TopCardData>>()),
                        })
                        .collect();
                    data.set_top_groups(model(groups));
                    data.set_top_state(2);
                    // 榜单卡换图（T5/D-8：daemon picurl → CoverGet；响应
                    // 同帧落地，URL 直传无 side-table，迟到图无匹配即丢弃）
                    crate::online_covers::refresh_top_covers(
                        ui.as_weak(),
                        &rt2,
                        page.groups
                            .iter()
                            .flat_map(|g| g.tops.iter())
                            .filter(|t| !t.picurl.is_empty())
                            .map(|t| (t.id.to_string(), t.picurl.clone()))
                            .collect(),
                    );
                }
                Ok(hmp_core::Response::TopCategory(_)) => {
                    data.set_top_state(3);
                }
                _ => {
                    data.set_top_state(4);
                }
            }
        });
    });
}

/// 榜单卡投影（TopSummaryDto → TopCardData；T5 单测锚定）。封面程序化
/// 占位先行（seed top:{id}），daemon picurl 经 CoverGet 异步换真图。
fn top_card(t: &hmp_core::TopSummaryDto) -> TopCardData {
    TopCardData {
        id: t.id.to_string().into(),
        name: t.name.as_str().into(),
        title_sub: t.title_sub.as_str().into(),
        update_time: t.update_time.as_str().into(),
        listen_num: t.listen_num as i32,
        total_num: t.total_num as i32,
        preview: model(
            t.preview
                .iter()
                .map(|s| s.as_str().into())
                .collect::<Vec<slint::SharedString>>(),
        ),
        cover: crate::covers::cover_image(&format!("top:{}", t.id)),
    }
}

/// 账号状态查询（daemon AccountStatus IPC；设置页账号面板）。
/// daemon 冷启动（连接失败）时重试：首次连接由订阅循环 connect_or_spawn
/// 拉起 daemon 需 ~1-3s，这里的小重试覆盖该窗口；彻底离线 → 专属失败态
/// （区别于"未登录"，不误导用户去重新登录）。
/// 启动查一次 + on_navigate 命中 SettingsAccount 时重发（重进页面刷新，
/// 覆盖 daemon 后启动 / 终端 `hmp login` 后的状态变化）。
fn spawn_account_status_load(
    ui_weak: Weak<AppWindow>,
    runtime: &Arc<crate::backend::BackendRuntime>,
) {
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
        let _ = slint::invoke_from_event_loop(move || {
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
        });
    });
}

/// daemon 扫码状态 → UI login-state（1=等待扫码 2=已扫码 3=成功 4=已拒绝 5=失败）。
fn login_ui_state(wire_status: i32) -> i32 {
    match wire_status {
        hmp_core::ipc::LoginQrState::STATUS_WAITING => 1,
        hmp_core::ipc::LoginQrState::STATUS_SCANNED => 2,
        hmp_core::ipc::LoginQrState::STATUS_DONE => 3,
        hmp_core::ipc::LoginQrState::STATUS_REFUSED => 4,
        _ => 5,
    }
}

/// 加载登录二维码（daemon 已落盘的本机 PNG；UI 禁 HTTP，AUDIT §8.4）。
fn load_login_qr_image(path: &str) -> Option<slint::Image> {
    if path.is_empty() {
        return None;
    }
    slint::Image::load_from_path(std::path::Path::new(path)).ok()
}

/// 登录轮询循环：每 1.5s 一次 `LoginQrPoll`，终态（成功/拒绝/失败/无会话）
/// 即停；二维码超时自动刷新 → 路径变化 → 重载图。过代（新会话/取消）静默退出。
fn spawn_login_poll_loop(
    ui_weak: Weak<AppWindow>,
    runtime: Arc<crate::backend::BackendRuntime>,
    generation: u64,
    mut qr_path: String,
) {
    let spawn_rt = Arc::clone(&runtime);
    spawn_rt.spawn(async move {
        loop {
            tokio::time::sleep(LOGIN_POLL_INTERVAL).await;
            if LOGIN_GENERATION.load(Ordering::Relaxed) != generation {
                return;
            }
            let result = crate::backend::request(hmp_core::Request::LoginQrPoll).await;
            // 终态判定基于纯数据（Send）；UI 更新统一回事件循环线程。
            let terminal = match &result {
                Ok(hmp_core::Response::LoginQrState(state)) => !matches!(
                    state.status,
                    hmp_core::ipc::LoginQrState::STATUS_WAITING
                        | hmp_core::ipc::LoginQrState::STATUS_SCANNED
                ),
                _ => true,
            };
            if let Ok(hmp_core::Response::LoginQrState(state)) = &result {
                if !state.qr_path.is_empty() && state.qr_path != qr_path {
                    qr_path = state.qr_path.clone();
                }
            }
            let weak = ui_weak.clone();
            let rt = Arc::clone(&runtime);
            let current_path = qr_path.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                if LOGIN_GENERATION.load(Ordering::Relaxed) != generation {
                    return;
                }
                let data = Data::get(&ui);
                match result {
                    Ok(hmp_core::Response::LoginQrState(state)) => {
                        if !state.qr_path.is_empty() && state.qr_path != current_path {
                            if let Some(img) = load_login_qr_image(&state.qr_path) {
                                data.set_login_qr(img);
                            }
                        }
                        data.set_login_message(state.message.clone().into());
                        data.set_login_state(login_ui_state(state.status));
                        if state.status == hmp_core::ipc::LoginQrState::STATUS_DONE {
                            // 登录成功即重查账号态：昵称/VIP 面板与猜你喜欢解锁
                            // 都以 AccountStatus 为准（login-state=3 只是过渡展示）。
                            spawn_account_status_load(ui.as_weak(), &rt);
                        }
                    }
                    _ => {
                        data.set_login_state(5);
                        data.set_login_message("与守护进程通信失败，登录中止".into());
                    }
                }
            });
            if terminal {
                return;
            }
        }
    });
}

/// 把媒体库快照（直读 library.sqlite3，离线降级为空）装载进 Data global：
/// 五个库页真数据；下载/已购两页后端无对应域，渲染为诚实空态。
pub fn load_data(ui: &AppWindow, runtime: &Arc<crate::backend::BackendRuntime>) {
    apply_snapshot(ui, runtime, &library_view::load_snapshot());
}

/// 库变更刷新（`Event::LibraryChanged` 驱动，AUDIT §8.9）：防抖合并 + 后台
/// 读库。SQLite 读移出 UI 线程（此前刷新在 UI 线程同步全量读，扫描期是
/// 按钮卡死来源）；模型落地（slint::Image 构造、回调重绑）仍回 UI 线程。
/// 详情页数据也后台算好——路由参数在 UI 线程取（Nav global 非 Send）。
pub fn schedule_refresh(ui: &AppWindow, runtime: &Arc<crate::backend::BackendRuntime>) {
    thread_local! {
        static TIMER: RefCell<slint::Timer> = RefCell::new(slint::Timer::default());
    }
    let ui_weak = ui.as_weak();
    let runtime = Arc::clone(runtime);
    TIMER.with(|cell| {
        let timer = cell.borrow();
        timer.stop();
        timer.start(slint::TimerMode::SingleShot, REFRESH_DEBOUNCE, move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let nav = Nav::get(&ui);
            let route = nav.get_route();
            let param = nav.get_param();
            let ui_weak = ui_weak.clone();
            let rt = Arc::clone(&runtime);
            runtime.spawn(async move {
                // 阻塞 rusqlite 在 runtime 工作线程执行（与 backend 队列
                // 投影同惯例；投影数据均为纯数据，满足 Send）。
                let snap = library_view::load_snapshot();
                let detail = load_detail(route, &param);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak.upgrade() else {
                        return;
                    };
                    apply_snapshot(&ui, &rt, &snap);
                    apply_detail(&ui, &rt, &detail);
                });
            });
        });
    });
}

fn apply_snapshot(
    ui: &AppWindow,
    runtime: &Arc<crate::backend::BackendRuntime>,
    snap: &library_view::Snapshot,
) {
    let data = Data::get(ui);

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

    // ——— 最近播放（latest/earliest 已按真实时间戳格式化）———
    data.set_recent(model(
        snap.recent
            .iter()
            .map(SongRow::to_track_row)
            .collect::<Vec<_>>(),
    ));
    data.set_recent_count(snap.recent.len() as i32);
    data.set_recent_latest(snap.recent_latest.clone().into());
    data.set_recent_earliest(snap.recent_earliest.clone().into());

    // 曲目封面预取：QQ 行 remote_cover 批量经 daemon CoverGet 换本地产物
    // （打开列表即取图，命中 daemon 盘缓存零出网；回包广播回填各列表）。
    crate::track_covers::prefetch_tracks(
        runtime,
        crate::track_covers::targets_from_rows(&snap.liked)
            .into_iter()
            .chain(crate::track_covers::targets_from_rows(&snap.recent))
            .collect(),
    );

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

    // ——— 下载/已购两页：后端尚无下载/已购域（docs/AUDIT.md M8 记录）———
    // 诚实空态：不以虚构曲目填充，页面据 count=0 显示空态文案；
    // 数据域落地后此处改为真实投影。
    data.set_downloads(model(Vec::new()));
    data.set_downloads_count(0);
    data.set_downloads_size("—".into());
    data.set_downloads_lossless(0);
    data.set_downloads_storage_path("—".into());
    data.set_purchased_singles(model(Vec::new()));
    data.set_purchased_albums(model(Vec::new()));
    data.set_purchased_single_count(0);
    data.set_purchased_album_count(0);
    data.set_purchased_total("—".into());
}

/// 路由落地后的详情装载（导航路径：单次同步读，单页查询量小；库变更刷新
/// 路径走后台预计算 + [`apply_detail`]）。本地 miss 即诚实空态（state=3）。
/// 远端参数（`qq:` 前缀）在本地 miss 后分流到异步远端装载（D-1）。
fn apply_route(
    ui: &AppWindow,
    runtime: &Arc<crate::backend::BackendRuntime>,
    route: crate::Route,
    param: SharedString,
) {
    let detail = load_detail(route, &param);
    // 远端分流：本地投影 miss 且带 qq: 前缀 → 异步远端装载（本地事实
    // 优先，R3：同形展示名本地命中时不会走到这里）。load_detail 对远端
    // 参数返回 None，miss 判定涵盖。
    if matches!(
        detail,
        DetailData::None
            | DetailData::Playlist(_, None)
            | DetailData::Album(None)
            | DetailData::Artist(None)
    ) {
        let remote_mid = crate::remote_details::remote_param(&param);
        let handled = match (route, remote_mid) {
            (crate::Route::Playlist, Some(_)) => {
                crate::remote_details::spawn_remote_playlist_load(ui.as_weak(), runtime, &param);
                true
            }
            (crate::Route::Album, Some(_)) => {
                crate::remote_details::spawn_remote_album_load(ui.as_weak(), runtime, &param);
                true
            }
            (crate::Route::Artist, Some(_)) => {
                crate::remote_details::spawn_remote_artist_load(ui.as_weak(), runtime, &param);
                true
            }
            _ => false,
        };
        if handled {
            return;
        }
    }
    apply_detail(ui, runtime, &detail);
}

/// 详情页数据（纯数据、Send——供库变更刷新在后台线程预算）。
/// `Playlist` 携带 id 串（封面 seed 用，与库页歌单卡同一实体身份）。
enum DetailData {
    Playlist(String, Option<library_view::PlaylistDetail>),
    Album(Option<library_view::AlbumDetail>),
    Artist(Option<library_view::ArtistDetail>),
    None,
}

fn load_detail(route: crate::Route, param: &str) -> DetailData {
    match route {
        crate::Route::Playlist => {
            if let Some(detail) = param
                .parse::<i64>()
                .ok()
                .and_then(library_view::playlist_detail)
            {
                return DetailData::Playlist(param.to_string(), Some(detail));
            }
            if crate::remote_details::remote_param(param).is_some() {
                // 远端参数：本地投影 miss（数字 parse 被前缀挡住必 miss）。
                // 返回 None → 库变更重放（schedule_refresh）apply_detail
                // no-op，远端页面数据不被库变更误清（D-1）；导航路径由
                // apply_route 在此之前分流到远端装载。
                return DetailData::None;
            }
            DetailData::Playlist(param.to_string(), None)
        }
        // 专辑/歌手（展示名参数）：本地命中优先（R3——展示名理论上可叫
        // "qq:…" 属极端构造，本地事实先行）；miss 且带前缀 → None（同上）。
        crate::Route::Album => {
            if let Some(detail) = library_view::album_detail(param) {
                DetailData::Album(Some(detail))
            } else if crate::remote_details::remote_param(param).is_some() {
                DetailData::None
            } else {
                DetailData::Album(None)
            }
        }
        crate::Route::Artist => {
            if let Some(detail) = library_view::artist_detail(param) {
                DetailData::Artist(Some(detail))
            } else if crate::remote_details::remote_param(param).is_some() {
                DetailData::None
            } else {
                DetailData::Artist(None)
            }
        }
        _ => DetailData::None,
    }
}

fn apply_detail(
    ui: &AppWindow,
    runtime: &Arc<crate::backend::BackendRuntime>,
    detail: &DetailData,
) {
    let data = Data::get(ui);
    match detail {
        DetailData::Playlist(param, Some(detail)) => {
            // 本地命中 → 就绪（M2：本地路径同步维护 state，防「远端失败
            // state=4 → 返回本地歌单仍渲染失败文案」的交替导航残留）
            data.set_playlist_state(2);
            data.set_playlist_found(true);
            data.set_playlist_name(detail.name.clone().into());
            data.set_playlist_meta_items(model(vec![
                format!("{} 首", detail.tracks.len()).into(),
                format!("总时长 {}", format_long_duration(detail.total_ms)).into(),
            ]));
            // 真封面优先（reconcile 回写的本地产物），缺失回退程序化占位
            data.set_playlist_cover(
                local_cover_image(detail.cover_uri.as_deref())
                    .unwrap_or_else(|| cover_image(&format!("playlist:{param}"))),
            );
            data.set_playlist_tracks(model(
                detail.tracks.iter().map(SongRow::to_track_row).collect(),
            ));
            // 打开歌单即预取曲目封面（用户预期：进列表就该看到封面而非
            // 占位盘）；daemon 盘缓存命中零出网，回包广播回填。
            crate::track_covers::prefetch_tracks(
                runtime,
                crate::track_covers::targets_from_rows(&detail.tracks),
            );
        }
        // 本地 miss → 未找到（M2：同步维护 state）
        DetailData::Playlist(_, None) => {
            data.set_playlist_state(3);
            data.set_playlist_found(false);
        }
        DetailData::Album(Some(detail)) => {
            data.set_album_state(2);
            data.set_album_found(true);
            data.set_album_name(detail.name.clone().into());
            data.set_album_artist(detail.artist.clone().unwrap_or_default().into());
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
            crate::track_covers::prefetch_tracks(
                runtime,
                crate::track_covers::targets_from_rows(&detail.tracks),
            );
        }
        DetailData::Album(None) => {
            data.set_album_state(3);
            data.set_album_found(false);
        }
        DetailData::Artist(Some(detail)) => {
            data.set_artist_state(2);
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
            crate::track_covers::prefetch_tracks(
                runtime,
                crate::track_covers::targets_from_rows(&detail.tracks),
            );
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
        DetailData::Artist(None) => {
            data.set_artist_state(3);
            data.set_artist_found(false);
        }
        DetailData::None => {}
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
    // 导航时在线内容页懒加载所需 runtime（克隆进闭包）。
    let nav_rt = Arc::clone(&runtime);
    // 猜你喜欢触发器（在 on_refresh_guess 绑定后填充，避免闭包循环依赖）。
    type GuessTrigger = Box<dyn Fn(Weak<AppWindow>) + Send>;
    let guess_trigger: Arc<Mutex<GuessTrigger>> = Arc::new(Mutex::new(Box::new(|_| {})));
    {
        let history = Arc::clone(&history);
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let rt = Arc::clone(&nav_rt);
        let guess_trigger = Arc::clone(&guess_trigger);
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
            nav.set_param(param.clone());
            nav.set_can_go_back(!history.lock().expect("nav history").is_empty());
            apply_route(&ui, &rt, nav.get_route(), nav.get_param());
            // M4 在线内容页进入时懒加载（免登录页自动拉，猜你喜欢需登录）
            match route {
                crate::Route::Home => {
                    spawn_discover_load(ui.as_weak(), &rt);
                    guess_trigger.lock().expect("guess trigger")(ui.as_weak());
                }
                crate::Route::Discover => {
                    spawn_discover_load(ui.as_weak(), &rt);
                }
                crate::Route::Top => {
                    spawn_top_category_load(ui.as_weak(), &rt);
                }
                crate::Route::TopDetail => {
                    spawn_top_detail_load(ui.as_weak(), &rt, param);
                }
                // 账号页重进即重查（daemon 后启动 / `hmp login` 后刷新状态）
                crate::Route::SettingsAccount => {
                    spawn_account_status_load(ui.as_weak(), &rt);
                }
                _ => {}
            }
        });
    }
    {
        let history = Arc::clone(&history);
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let rt = Arc::clone(&nav_rt);
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
            apply_route(&ui, &rt, nav.get_route(), nav.get_param());
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
            // 曲目层取色的亮暗目标窗不同，整族随模式重算（无取色时空操作）
            crate::track_theme::reapply_for_theme(&ui);
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
            crate::track_theme::reapply_for_theme(&ui);
            prefs.lock().expect("prefs").theme_mode = mode;
            crate::prefs::store(&prefs.lock().expect("prefs").clone());
        });
    }

    // ——— 系统偏好漂移（mode=跟随系统时 Palette.color-scheme 变化）———
    // Theme.dark 的 changed 回调进 Rust 重取曲目层调色；用户显式切档走上面
    // 两条路径，这里只兜系统侧变化（无取色时空操作）
    {
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        Theme::get(ui).on_retheme(move || {
            if let Some(ui) = ui_weak.upgrade() {
                crate::track_theme::reapply_for_theme(&ui);
            }
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
            // 每次真实搜索递增代次：空词早退不动计数，不打断在途请求
            let generation = SEARCH_GENERATION.with(|g| {
                g.set(g.get() + 1);
                g.get()
            });
            if let Some(ui) = ui_weak.upgrade() {
                let data = Data::get(&ui);
                data.set_search_query(SharedString::from(keyword.clone()));
                data.set_search_state(1);
            }
            let ui_weak = ui_weak.clone();
            let rt2 = Arc::clone(&runtime);
            runtime.spawn(async move {
                let result = crate::backend::request(hmp_core::Request::Search { keyword }).await;
                let rt2 = Arc::clone(&rt2);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak.upgrade() else {
                        return;
                    };
                    // 非最新代次 = 更晚的搜索已发出，本次响应过期丢弃
                    // （"搜索中"态由最新请求置位，此处不动状态）
                    if SEARCH_GENERATION.with(|g| g.get()) != generation {
                        return;
                    }
                    let data = Data::get(&ui);
                    match result {
                        Ok(hmp_core::Response::Search(page)) => {
                            let rows: Vec<TrackRow> =
                                page.songs.iter().map(search_track_row).collect();
                            // C6：专辑/歌手组同帧落地（此前消费侧丢弃）；
                            // 计数徽章由 UI 直接绑模型长度（M4 零桥接成本）
                            let albums: Vec<CoverCardData> =
                                page.albums.iter().map(search_album_card).collect();
                            let artists: Vec<crate::ArtistCardData> =
                                page.singers.iter().map(search_singer_card).collect();
                            // 无结果 = 三组全空（C6；此前只看歌曲组）
                            let found = search_has_results(&rows, &albums, &artists);
                            // search-count 保持歌曲组总数语义（「共 N 条」行）
                            data.set_search_count(rows.len() as i32);
                            data.set_search_results(model(rows));
                            data.set_search_albums(model(albums));
                            data.set_search_artists(model(artists));
                            data.set_search_state(if found { 2 } else { 3 });
                            // 专辑/歌手卡换图：响应同帧落地模型，URL 随投影持有
                            // （无 side-table；迟到图无匹配行即丢弃）
                            crate::online_covers::refresh_search_covers(
                                ui.as_weak(),
                                &rt2,
                                page.albums
                                    .iter()
                                    .filter(|a| !a.pic.is_empty())
                                    .map(|a| (a.mid.clone(), a.pic.clone()))
                                    .collect(),
                                page.singers
                                    .iter()
                                    .filter(|s| !s.pic.is_empty())
                                    .map(|s| (s.mid.clone(), s.pic.clone()))
                                    .collect(),
                            );
                        }
                        // 离线/daemon 错误/协议错误统一失败态（页面文案覆盖）。
                        _ => data.set_search_state(4),
                    }
                });
            });
        });
    }

    // ——— 账号状态（daemon AccountStatus IPC；设置页账号面板）———
    // 启动查一次；重进账号页时由 on_navigate 重发（见 spawn_account_status_load）
    spawn_account_status_load(ui.as_weak(), &runtime);

    // ——— 扫码登录 / 退出登录（daemon LoginQr*/Logout IPC）———
    {
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let runtime = Arc::clone(&runtime);
        Data::get(ui).on_start_login(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // bump 代次：已在进行的轮询循环即刻失效，daemon 侧 Start 换新会话
            let generation = LOGIN_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
            {
                let data = Data::get(&ui);
                data.set_login_state(1);
                data.set_login_message("正在生成二维码…".into());
            }
            let weak = ui_weak.clone();
            let rt = Arc::clone(&runtime);
            runtime.spawn(async move {
                let result = crate::backend::request(hmp_core::Request::LoginQrStart).await;
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else { return };
                    if LOGIN_GENERATION.load(Ordering::Relaxed) != generation {
                        return;
                    }
                    let data = Data::get(&ui);
                    match result {
                        Ok(hmp_core::Response::LoginQr(session)) => {
                            if let Some(img) = load_login_qr_image(&session.qr_path) {
                                data.set_login_qr(img);
                            }
                            data.set_login_message(String::new().into());
                            data.set_login_state(1);
                            spawn_login_poll_loop(ui.as_weak(), rt, generation, session.qr_path);
                        }
                        _ => {
                            data.set_login_state(5);
                            data.set_login_message("无法开始扫码登录（守护进程不可达）".into());
                        }
                    }
                });
            });
        });
    }
    {
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let runtime = Arc::clone(&runtime);
        Data::get(ui).on_cancel_login(move || {
            // bump 代次终止轮询 + 复位面板；daemon 会话取消尽力而为
            LOGIN_GENERATION.fetch_add(1, Ordering::Relaxed);
            if let Some(ui) = ui_weak.upgrade() {
                let data = Data::get(&ui);
                data.set_login_state(0);
                data.set_login_message(String::new().into());
            }
            runtime.spawn(async move {
                let _ = crate::backend::request(hmp_core::Request::LoginQrCancel).await;
            });
        });
    }
    {
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let runtime = Arc::clone(&runtime);
        Data::get(ui).on_logout(move || {
            // 扫码面板乐观复位；账号态以 Logout 后的 AccountStatus 重查为准
            LOGIN_GENERATION.fetch_add(1, Ordering::Relaxed);
            if let Some(ui) = ui_weak.upgrade() {
                let data = Data::get(&ui);
                data.set_login_state(0);
                data.set_login_message(String::new().into());
            }
            let weak = ui_weak.clone();
            let rt = Arc::clone(&runtime);
            runtime.spawn(async move {
                let _ = crate::backend::request(hmp_core::Request::Logout).await;
                spawn_account_status_load(weak, &rt);
            });
        });
    }

    // ——— M4 在线内容页（DiscoverGet/TopCategoryGet/TopDetailGet/GuessGet）———
    // 发现页：免登录；进入页面或点刷新时拉取。runtime 供后台出网。
    {
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let rt = Arc::clone(&runtime);
        Data::get(ui).on_refresh_discover(move || {
            spawn_discover_load(ui_weak.clone(), &rt);
        });
    }
    // 排行榜分类：免登录。
    {
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let rt = Arc::clone(&runtime);
        Data::get(ui).on_refresh_top(move || {
            spawn_top_category_load(ui_weak.clone(), &rt);
        });
    }
    // 猜你喜欢：需登录；未登录直接置提示态，不发请求。
    {
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let rt = Arc::clone(&runtime);
        Data::get(ui).on_refresh_guess(move || {
            let apply_state = move |state: i32, weak: Weak<AppWindow>| {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        Data::get(&ui).set_guess_state(state);
                    }
                });
            };
            // 登录态在点击时读实时值：bind 时刻账号查询（异步）尚未完成，
            // 恒 false——捕获进闭包会让「刷新猜你喜欢」在会话中途 `hmp login`
            // 后永远误报未登录（2026-09-29 失效接线审计）。
            let logged_in = ui_weak
                .upgrade()
                .map(|ui| Data::get(&ui).get_account_logged_in())
                .unwrap_or(false);
            if !logged_in {
                apply_state(3, ui_weak.clone());
                return;
            }
            apply_state(1, ui_weak.clone());
            let weak = ui_weak.clone();
            let rt2 = Arc::clone(&rt);
            rt.spawn(async move {
                let result = crate::backend::request(hmp_core::Request::GuessGet { page: 1 }).await;
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else { return };
                    let data = Data::get(&ui);
                    match result {
                        Ok(hmp_core::Response::Guess(page)) if !page.songs.is_empty() => {
                            let tracks: Vec<TrackRow> =
                                page.songs.iter().map(discover_track_row).collect();
                            let cards: Vec<CoverCardData> = page
                                .songs
                                .iter()
                                .map(|s| CoverCardData {
                                    mid: s.mid.clone().into(),
                                    title: s.name.clone().into(),
                                    subtitle: s.singer.clone().into(),
                                    cover: crate::covers::cover_image(&format!("album:{}", s.mid)),
                                })
                                .collect();
                            data.set_guess_tracks(model(tracks));
                            data.set_guess_cards(model(cards));
                            data.set_guess_state(2);
                            // 猜你喜欢行封面预取（同 discover 新歌行）
                            crate::track_covers::prefetch_tracks(
                                &rt2,
                                page.songs
                                    .iter()
                                    .filter(|s| !s.picurl.is_empty())
                                    .map(|s| (s.mid.clone(), s.picurl.clone()))
                                    .collect(),
                            );
                        }
                        Ok(hmp_core::Response::Guess(_)) => {
                            data.set_guess_state(3);
                        }
                        _ => {
                            data.set_guess_state(4);
                        }
                    }
                });
            });
        });
    }
    // 填充猜你喜欢触发器：导航到首页时若已登录则触发一次拉取
    // （复用 on_refresh_guess 闭包太重，这里直接发 GuessGet）。
    {
        let rt_guess = Arc::clone(&nav_rt);
        *guess_trigger.lock().expect("guess trigger") = Box::new(move |weak: Weak<AppWindow>| {
            let weak_state = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak_state.upgrade() {
                    if !Data::get(&ui).get_account_logged_in() {
                        Data::get(&ui).set_guess_state(3);
                    }
                }
            });
            let rt = Arc::clone(&rt_guess);
            let rt2 = Arc::clone(&rt);
            rt.spawn(async move {
                let result = crate::backend::request(hmp_core::Request::GuessGet { page: 1 }).await;
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else { return };
                    let data = Data::get(&ui);
                    if !data.get_account_logged_in() {
                        return;
                    }
                    match result {
                        Ok(hmp_core::Response::Guess(page)) if !page.songs.is_empty() => {
                            let tracks: Vec<TrackRow> =
                                page.songs.iter().map(discover_track_row).collect();
                            data.set_guess_tracks(model(tracks));
                            data.set_guess_state(2);
                            // 登录后自动触发的猜你喜欢装载同款预取
                            crate::track_covers::prefetch_tracks(
                                &rt2,
                                page.songs
                                    .iter()
                                    .filter(|s| !s.picurl.is_empty())
                                    .map(|s| (s.mid.clone(), s.picurl.clone()))
                                    .collect(),
                            );
                        }
                        Ok(hmp_core::Response::Guess(_)) => {
                            data.set_guess_state(3);
                        }
                        _ => {
                            data.set_guess_state(4);
                        }
                    }
                });
            });
        });
    }
    // 注：不再在此预载发现页——bind 时事件循环尚未迭代，TestingBackend 下首个
    // invoke_from_event_loop 闭包会挂住加载任务；统一由导航懒加载（事件泵已运转）触发。
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
            cover_uri: None,
            relation: "local".into(),
        };
        let a = sidebar_cover(&entry);
        let b = sidebar_cover(&entry);
        assert_eq!(a.id, b.id, "同实体 id 稳定");
        assert_eq!(a.name, "深夜循环");
        assert_eq!(a.relation, "local");
        assert_eq!(a.image.size().width, b.image.size().width, "占位封面确定性");
        assert!(a.image.size().width > 0, "占位封面图非空");
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
        // TrackRow 已剥 cover（表格行不背位图，MEMFIX 工作②）：无封面断言可做，
        // 字段层面的占位图职责移交 QueueRow（player_bridge 队列投影）
    }

    /// C6：搜索专辑/歌手卡投影（qq: 前缀直达远端详情）与三组全空判定。
    #[test]
    fn search_cards_prefix_links_and_all_empty_verdict() {
        let album = hmp_core::SearchAlbum {
            mid: "002albUMZTyiKq".into(),
            name: "范特西".into(),
            singer: "周杰伦".into(),
            pic: String::new(),
        };
        let singer = hmp_core::SearchSinger {
            mid: "0025NhlN2yWrP4".into(),
            name: "周杰伦".into(),
            pic: String::new(),
        };
        let album_card = search_album_card(&album);
        assert_eq!(album_card.mid.as_str(), "qq:002albUMZTyiKq");
        assert_eq!(album_card.title.as_str(), "范特西");
        assert_eq!(album_card.subtitle.as_str(), "周杰伦");
        let singer_card = search_singer_card(&singer);
        assert_eq!(singer_card.mid.as_str(), "qq:0025NhlN2yWrP4");
        assert_eq!(singer_card.name.as_str(), "周杰伦");

        // 三组全空才判无结果（C6；此前只看歌曲组）
        assert!(search_has_results(&[TrackRow::default()], &[], &[]));
        assert!(search_has_results(&[], &[album_card], &[]));
        assert!(search_has_results(&[], &[], &[singer_card]));
        assert!(!search_has_results(&[], &[], &[]));
    }

    /// T5 验收：TopCardData 投影含 picurl→占位与 total_num 透传。
    #[test]
    fn top_card_projects_cover_placeholder_and_total_num() {
        let t = hmp_core::TopSummaryDto {
            id: 26,
            name: "热歌榜".into(),
            title_sub: "".into(),
            update_time: "2026-10-23".into(),
            listen_num: 32_500_000,
            picurl: "https://y.gtimg.cn/top.jpg".into(),
            total_num: 300,
            preview: vec!["1. 歌一 - 歌手甲".into()],
        };
        let card = top_card(&t);
        assert_eq!(card.id.as_str(), "26");
        assert_eq!(card.name.as_str(), "热歌榜");
        assert_eq!(card.total_num, 300, "B3「N 首」角标数据");
        assert!(card.cover.size().width > 0, "picurl → 程序化占位先行");
    }

    /// D-1 分流：远端参数（qq:）→ `DetailData::None`（导航分流信号 + 库变更
    /// 重放 no-op）；本地参数形态（数字 id / 展示名）不误判（miss 走诚实空态）。
    #[test]
    fn load_detail_routes_remote_params_to_none() {
        assert!(matches!(
            load_detail(crate::Route::Playlist, "qq:8522132521"),
            DetailData::None
        ));
        assert!(matches!(
            load_detail(crate::Route::Album, "qq:002albUMZTyiKq"),
            DetailData::None
        ));
        assert!(matches!(
            load_detail(crate::Route::Artist, "qq:0025NhlN2yWrP4"),
            DetailData::None
        ));
        // 本地参数：形态保持（命中与否取决于本机库，只锚定不误判远端）
        match load_detail(crate::Route::Playlist, "123") {
            DetailData::Playlist(p, _) => assert_eq!(p, "123"),
            _ => panic!("本地数字 id 参数必须保持 Playlist 形态"),
        }
        assert!(matches!(
            load_detail(crate::Route::Album, "十一月的萧邦"),
            DetailData::Album(_)
        ));
        assert!(matches!(
            load_detail(crate::Route::Artist, "周杰伦"),
            DetailData::Artist(_)
        ));
    }

    /// M1 验收：推荐卡 mid（qq: 前缀形态）↔ 封面旁路表键一致——卡片构造与
    /// 登记两侧同走 `playlist_card_key`，任一侧改形即在此失配。
    #[test]
    fn discover_card_mid_prefix_matches_cover_side_table() {
        let p = hmp_core::DiscoverPlaylist {
            id: 7468450811,
            title: "爆款预警".into(),
            picurl: "https://y.gtimg.cn/cover.jpg".into(),
            creator: "胡桃".into(),
            songnum: 161,
            listennum: 0,
        };
        let card = discover_playlist_card(&p);
        // 卡片 mid 带前缀（点击即远端详情动线，D-1）
        assert_eq!(card.mid.as_str(), "qq:7468450811");
        // 登记侧（裸 id）与卡片 mid 命中同一键（M1）
        crate::online_covers::register_playlist_covers(vec![(
            p.id.to_string(),
            "https://y.gtimg.cn/cover.jpg".into(),
        )]);
        assert_eq!(
            crate::online_covers::playlist_cover_of(card.mid.as_str()).as_deref(),
            Some("https://y.gtimg.cn/cover.jpg"),
            "发现卡 mid 必须命中封面旁路表键"
        );
    }

    /// M2 验收：本地/远端交替导航后状态正确——远端失败 state=4 → 返回本地
    /// 命中 state=2 正常渲染；本地 miss → state=3（未找到/失败不串台）。
    #[test]
    fn local_remote_alternating_navigation_states() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let runtime = Arc::new(crate::backend::BackendRuntime::new().unwrap());
        let data = Data::get(&ui);

        // 远端回包失败（离线）→ state=4
        crate::remote_details::apply_playlist_response(
            &ui.as_weak(),
            &runtime,
            crate::remote_details::current_playlist_generation(),
            Err(crate::backend::BackendError::Protocol("offline".into())),
        );
        assert_eq!(data.get_playlist_state(), 4, "远端失败态");

        // 返回本地歌单（本地命中）→ state=2、found=true 正常渲染
        let detail = DetailData::Playlist(
            "3".into(),
            Some(library_view::PlaylistDetail {
                name: "深夜循环".into(),
                tracks: vec![],
                total_ms: 0,
                cover_uri: None,
            }),
        );
        apply_detail(&ui, &runtime, &detail);
        assert_eq!(data.get_playlist_state(), 2, "本地命中必须复位就绪态");
        assert!(data.get_playlist_found());
        assert_eq!(data.get_playlist_name().as_str(), "深夜循环");

        // 本地 miss（已删除）→ state=3
        apply_detail(&ui, &runtime, &DetailData::Playlist("999".into(), None));
        assert_eq!(data.get_playlist_state(), 3, "本地 miss → 未找到");

        // 专辑/歌手本地分支同步置位（M2 逐一落位）
        apply_detail(&ui, &runtime, &DetailData::Album(None));
        assert_eq!(data.get_album_state(), 3);
        apply_detail(&ui, &runtime, &DetailData::Artist(None));
        assert_eq!(data.get_artist_state(), 3);

        // 远端专辑（T5）：失败 → 4；本地命中复位 2（交替导航，M2 同款）
        crate::remote_details::apply_album_response(
            &ui.as_weak(),
            &runtime,
            crate::remote_details::current_album_generation(),
            Err(crate::backend::BackendError::Protocol("offline".into())),
        );
        assert_eq!(data.get_album_state(), 4, "远端专辑失败态");
        let album = DetailData::Album(Some(library_view::AlbumDetail {
            name: "十一月的萧邦".into(),
            artist: Some("周杰伦".into()),
            year: Some(2005),
            cover_uri: None,
            tracks: vec![],
            total_ms: 0,
        }));
        apply_detail(&ui, &runtime, &album);
        assert_eq!(data.get_album_state(), 2, "本地专辑命中复位就绪态");

        // 远端歌手（T6）：失败 → 4；本地命中复位 2（交替导航，M2 同款）
        crate::remote_details::apply_artist_response(
            &ui.as_weak(),
            &runtime,
            crate::remote_details::current_artist_generation(),
            Err(crate::backend::BackendError::Protocol("offline".into())),
        );
        assert_eq!(data.get_artist_state(), 4, "远端歌手失败态");
        let artist = DetailData::Artist(Some(library_view::ArtistDetail {
            name: "周杰伦".into(),
            tracks: vec![],
            albums: vec![],
        }));
        apply_detail(&ui, &runtime, &artist);
        assert_eq!(data.get_artist_state(), 2, "本地歌手命中复位就绪态");
    }
}
