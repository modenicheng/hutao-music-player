//! Player global ↔ daemon 状态桥（M8，替换 M0-M3 的模拟播放桥）：
//! `DaemonState` → Player 单向映射 + 命令回调 → `Request` 短连接。
//! daemon 是唯一播放后端（docs/PROJECT.md §8.6）：本模块不做任何本地推算
//! （进度/队列/能力全部来自推送），无 daemon（离线降级）时 Player 全空、
//! 命令 no-op。全部 UI 写发生在 UI 线程（回调与订阅投递闭包内）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Global, Model, ModelRc, TimerMode, VecModel, Weak};

use hmp_core::ipc::{DaemonState, Request, Response};
use hmp_core::{AudioQuality, PlayRequest, PlaybackState, PlaybackStatus, PlayerCommand, TrackId};

use crate::backend::{BackendRuntime, QueueRowMeta, UiStateEvent};
use crate::{AppWindow, Player, TrackRow};

/// seek 落点确认容忍：daemon 推送位置与目标差 ≤ 此值视为已生效
/// （播放中位置持续前进，容忍取推送周期量级）。
const SEEK_CONFIRM_TOLERANCE_MS: u64 = 600;
/// seek 落点钉住超时：daemon 未确认（离线 / seek 失败）也恢复跟随。
const SEEK_PIN_TIMEOUT: Duration = Duration::from_millis(1_000);
/// 音量本地回显窗口：窗口内 daemon 推送的旧音量不回写（拖动防打架）。
const VOLUME_ECHO_WINDOW: Duration = Duration::from_millis(500);
/// 音量 SetVolume IPC 节流最小间隔（mousemove 级调用合并 + 尾随补发）。
const VOLUME_IPC_INTERVAL: Duration = Duration::from_millis(100);
/// 音量偏好落盘合并间隔：停手后写一次，拖动中不逐次同步写盘。
const VOLUME_STORE_DELAY: Duration = Duration::from_millis(600);

/// 松手 seek 的落点（进度条拖拽只在松手时 seek 一次）：daemon 确认到点 /
/// 超时 / 换曲前，携带旧位置的推送不回写展示进度，避免回跳闪烁。
#[derive(Clone)]
struct PendingSeek {
    target_ms: u64,
    mid: String,
    deadline: Instant,
}

/// 拖拽/回显竞态抑制（全部 UI 线程读写，Mutex 只为在 bind 的闭包间共享）。
#[derive(Default)]
struct EchoGuard {
    pending_seek: Option<PendingSeek>,
    volume_echo_until: Option<Instant>,
}

impl EchoGuard {
    /// daemon 音量推送是否可回写（本地回显窗口已过）。
    fn volume_echo_expired(&self) -> bool {
        self.volume_echo_until.is_none_or(|until| Instant::now() >= until)
    }
}

/// 绑定 Player global 命令回调并挂载订阅任务（bootstrap：首连在订阅循环内
/// 完成，必要时经 connect_or_spawn 拉起 daemon；彻底失败降级离线模式）。
/// runtime 由调用方持有存活到 `ui.run()` 结束（订阅任务挂在上面）。
pub fn bind(ui: &AppWindow, runtime: Arc<BackendRuntime>, prefs: Arc<Mutex<crate::prefs::Prefs>>) {
    let player = Player::get(ui);
    // 初始音量取本地偏好（仅首帧/离线兜底；在线后以 daemon 推送为准）。
    player.set_volume(prefs.lock().expect("prefs").volume.clamp(0.0, 1.0));

    // 最近一次队列投影的 mid 列表（UI 线程读写：applier 写、play_at 读）。
    let queue_mids: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // 拖拽/回显竞态抑制：命令回调写、订阅回调读（均在 UI 线程）。
    let echo: Arc<Mutex<EchoGuard>> = Arc::new(Mutex::new(EchoGuard::default()));

    // —— 命令回调 → Request（命令-查询分离：受理即返回，结果经状态推送呈现；
    // 离线时短连接失败被丢弃 = 静默 no-op）———
    macro_rules! command_callback {
        ($name:ident, $req:expr) => {{
            let runtime = Arc::clone(&runtime);
            player.$name(move || {
                let runtime = Arc::clone(&runtime);
                runtime.spawn(async move {
                    let _ = crate::backend::request($req).await;
                });
            });
        }};
    }
    command_callback!(on_toggle_play, Request::Command(PlayerCommand::TogglePlay));
    command_callback!(on_next, Request::Command(PlayerCommand::Next));
    command_callback!(on_previous, Request::Command(PlayerCommand::Previous));
    command_callback!(on_clear_queue, Request::QueueClear { all: true });
    {
        let runtime = Arc::clone(&runtime);
        player.on_remove_at(move |index| {
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let request = Request::QueueRemove(index);
            runtime.spawn(async move {
                let _ = crate::backend::request(request).await;
            });
        });
    }
    {
        // 进度条松手 seek（拖拽全程纯本地回显，见 player-bar.slint）：立即把
        // 展示进度钉在落点并记录 PendingSeek——daemon 确认到点前旧位置推送
        // 不回写，杜绝回跳闪烁；seek 只发一次，不再有逐移动事件的 IPC 风暴。
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let runtime = Arc::clone(&runtime);
        let echo = Arc::clone(&echo);
        player.on_seek_percent(move |percent| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let player = Player::get(&ui);
            let percent = percent.clamp(0.0, 1.0);
            let duration_ms = player.get_duration_ms().max(0) as f32;
            let position_ms = (duration_ms * percent).round() as u64;
            player.set_position_ms(position_ms as i32);
            player.set_progress(if duration_ms > 0.0 { percent } else { 0.0 });
            echo.lock().expect("echo").pending_seek = Some(PendingSeek {
                target_ms: position_ms,
                mid: player.get_current_mid().to_string(),
                deadline: Instant::now() + SEEK_PIN_TIMEOUT,
            });
            let request = Request::Command(PlayerCommand::Seek(Duration::from_millis(position_ms)));
            runtime.spawn(async move {
                let _ = crate::backend::request(request).await;
            });
        });
    }

    {
        // 音量：本地立即回显（thumb/数值跟手）；SetVolume IPC 按 100ms 节流 +
        // 尾随补发、偏好落盘停手 600ms 合并一次——此前 mousemove 级调用逐次
        // 同步写盘 + 短连接，是音量拖动卡顿来源。daemon 推送的旧值在回显窗口
        // 内不回写（apply_daemon_state）。定时器/节流态全在 UI 线程（Timer 非
        // Send，以 Rc 挂进回调闭包保活）。
        let ui_weak: Weak<AppWindow> = ui.as_weak();
        let prefs_for_send = Arc::clone(&prefs);
        let prefs_for_store = Arc::clone(&prefs);
        let runtime = Arc::clone(&runtime);
        let echo = Arc::clone(&echo);
        let pending: Rc<RefCell<Option<f64>>> = Rc::new(RefCell::new(None));
        let last_sent: Rc<RefCell<Option<Instant>>> = Rc::new(RefCell::new(None));
        let ipc_timer = Rc::new(slint::Timer::default());
        let store_timer = Rc::new(slint::Timer::default());
        let send_pending: Rc<dyn Fn()> = Rc::new({
            let pending = Rc::clone(&pending);
            let last_sent = Rc::clone(&last_sent);
            let runtime = Arc::clone(&runtime);
            move || {
                if let Some(volume) = pending.borrow_mut().take() {
                    *last_sent.borrow_mut() = Some(Instant::now());
                    send_volume_ipc(&runtime, volume);
                }
            }
        });
        player.on_set_volume(move |volume| {
            let volume = f64::from(volume).clamp(0.0, 1.0);
            prefs_for_send.lock().expect("prefs").volume = volume as f32;
            if let Some(ui) = ui_weak.upgrade() {
                Player::get(&ui).set_volume(volume as f32);
            }
            echo.lock().expect("echo").volume_echo_until =
                Some(Instant::now() + VOLUME_ECHO_WINDOW);

            // IPC 节流：距上次发送不足间隔 → 记为待发并定尾随定时器，否则立即发
            *pending.borrow_mut() = Some(volume);
            let now = Instant::now();
            let since_send =
                last_sent.borrow().map(|t| now.saturating_duration_since(t));
            match since_send {
                Some(elapsed) if elapsed < VOLUME_IPC_INTERVAL => {
                    let callback = Rc::clone(&send_pending);
                    ipc_timer.start(TimerMode::SingleShot, VOLUME_IPC_INTERVAL - elapsed, move || callback());
                }
                _ => {
                    send_pending();
                }
            }

            // 偏好落盘：停手后写一次（拖动中不断重启合并）
            let prefs = Arc::clone(&prefs_for_store);
            store_timer.start(TimerMode::SingleShot, VOLUME_STORE_DELAY, move || {
                crate::prefs::store(&prefs.lock().expect("prefs").clone());
            });
        });
    }

    {
        // 播放列表入口：取 start 行按 source 映射播放源（清队列换源语义）。
        let runtime = Arc::clone(&runtime);
        player.on_play_tracks(move |tracks, start| {
            let count = tracks.row_count();
            let start = usize::try_from(start)
                .ok()
                .filter(|index| *index < count)
                .unwrap_or(0);
            let Some(row) = tracks.row_data(start) else {
                return;
            };
            let request = Request::Play(play_request_for(&row));
            runtime.spawn(async move {
                let _ = crate::backend::request(request).await;
            });
        });
    }

    {
        // 队列点击：跳到该位置播放（QueuePlayAt，队列不被单曲替换，AUDIT §8.8）。
        let runtime = Arc::clone(&runtime);
        player.on_play_at(move |index| {
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let request = Request::QueuePlayAt(index);
            runtime.spawn(async move {
                let _ = crate::backend::request(request).await;
            });
        });
    }

    bind_ui_toggles(ui);

    // —— 订阅：DaemonState 推送 → Player global（UI 线程应用）———
    let ui_weak: Weak<AppWindow> = ui.as_weak();
    let runtime_for_state = Arc::clone(&runtime);
    let echo_for_state = Arc::clone(&echo);
    let handler = Arc::new(move |event: UiStateEvent| {
        apply_event(&ui_weak, &runtime_for_state, &queue_mids, &echo_for_state, event);
    });
    crate::backend::spawn_state_subscription(&runtime, handler);
}

/// `row.source`（0=QQ 1=本地，UI 显式标记）→ 播放源（`PlayRequest` 分流）。
fn play_request_for(row: &TrackRow) -> PlayRequest {
    play_request_for_source(row.mid.as_ref(), row.source == 1)
}

/// SetVolume 命令（短连接；受理即返回，结果经状态推送呈现）。
fn send_volume_ipc(runtime: &Arc<BackendRuntime>, volume: f64) {
    runtime.spawn(async move {
        let _ = crate::backend::request(Request::Command(PlayerCommand::SetVolume(volume))).await;
    });
}

fn play_request_for_source(mid: &str, is_local: bool) -> PlayRequest {
    let id = TrackId::new(mid);
    if is_local {
        PlayRequest::Local(id)
    } else {
        PlayRequest::Track(id)
    }
}

/// 队列抽屉 / 播放页 overlay 的纯 UI 回写（不产生 IPC；与模拟桥行为一致）。
fn bind_ui_toggles(ui: &AppWindow) {
    let player = Player::get(ui);
    let ui_weak: Weak<AppWindow> = ui.as_weak();
    player.on_toggle_queue(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let player = Player::get(&ui);
            player.set_queue_visible(!player.get_queue_visible());
        }
    });
    let ui_weak: Weak<AppWindow> = ui.as_weak();
    player.on_hide_queue(move || {
        if let Some(ui) = ui_weak.upgrade() {
            Player::get(&ui).set_queue_visible(false);
        }
    });
    let ui_weak: Weak<AppWindow> = ui.as_weak();
    player.on_show_overlay(move || {
        if let Some(ui) = ui_weak.upgrade() {
            Player::get(&ui).set_overlay_visible(true);
        }
    });
    let ui_weak: Weak<AppWindow> = ui.as_weak();
    player.on_hide_overlay(move || {
        if let Some(ui) = ui_weak.upgrade() {
            Player::get(&ui).set_overlay_visible(false);
        }
    });
}

/// 订阅事件 → Player global（UI 线程执行）。
fn apply_event(
    ui_weak: &Weak<AppWindow>,
    runtime: &Arc<BackendRuntime>,
    queue_mids: &Mutex<Vec<String>>,
    echo: &Mutex<EchoGuard>,
    event: UiStateEvent,
) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    let player = Player::get(&ui);

    // 库内容变更（扫描/监听/reconcile/写命令落库）：重查 sqlite 刷新库页
    // 与当前详情页（AUDIT §8.9；刷新前状态展示保持旧模型，不闪空态）。
    if event.library_changed {
        crate::bridge::refresh(&ui);
        return;
    }

    // 队列模型：仅结构变化（revision 前进 / 离线清空）时重建；
    // position 推送（~10Hz）绝不触发重建。
    if let Some(rows) = &event.queue_rows {
        let mids: Vec<String> = rows.iter().map(|row| row.mid.clone()).collect();
        *queue_mids.lock().expect("queue mids") = mids;
        let track_rows: Vec<TrackRow> = rows.iter().map(row_from_meta).collect();
        let total_ms: u64 = rows.iter().map(|row| row.duration_ms.max(0) as u64).sum();
        player.set_queue_meta(
            format!(
                "{} 首 · 总时长 {}",
                rows.len(),
                crate::format::format_long_duration(total_ms)
            )
            .into(),
        );
        player.set_queue(ModelRc::new(VecModel::from(track_rows)));
    }

    let Some(state) = event.state else {
        apply_offline(&player);
        return;
    };
    let mids = queue_mids.lock().expect("queue mids").clone();
    apply_daemon_state(&player, &state, &mids, echo, ui_weak, runtime);
}

/// 离线（无 daemon / daemon 已退出）：播放态全空（音量保留本地值不重置）。
fn apply_offline(player: &Player) {
    player.set_playing(false);
    player.set_can_previous(false);
    player.set_can_next(false);
    player.set_queue_current_mid("".into());
    clear_now_playing(player);
}

/// DaemonState → Player global 单向映射（daemon 是唯一状态出口）。
fn apply_daemon_state(
    player: &Player,
    state: &DaemonState,
    queue_mids: &[String],
    echo: &Mutex<EchoGuard>,
    ui_weak: &Weak<AppWindow>,
    runtime: &Arc<BackendRuntime>,
) {
    let playback = &state.playback;
    player.set_playing(playback.status == PlaybackStatus::Playing);
    // 音量展示用用户原值（playback.volume 含 RG 补偿，回设会静默偏移，AUDIT §8.12）；
    // 本地回显窗口内（音量拖动中，推送仍带旧值）不回写，避免与 thumb 打架。
    let volume = if (playback.user_volume - 0.0).abs() > f64::EPSILON {
        playback.user_volume
    } else {
        playback.volume
    };
    if echo.lock().expect("echo").volume_echo_expired() {
        player.set_volume(volume.clamp(0.0, 1.0) as f32);
    }
    player.set_can_previous(state.caps.can_go_previous);
    player.set_can_next(state.caps.can_go_next);
    player.set_queue_current_mid(current_mid(state, queue_mids).into());

    let Some(track) = &playback.current else {
        echo.lock().expect("echo").pending_seek = None;
        clear_now_playing(player);
        return;
    };
    player.set_has_track(true);
    player.set_current_mid(track.id.0.as_str().into());
    player.set_duration_ms(playback.duration.as_ref().map(duration_ms_i32).unwrap_or(0));

    // 进度展示：拖拽中（seeking）或落点未确认（松手 seek 后旧位置推送在途）
    // 时冻结展示值。拖拽开始即清落点（松手时会重新钉）；确认到点 / 超时 /
    // 换曲则恢复跟随。
    let mut progress_pinned = false;
    {
        let mut guard = echo.lock().expect("echo");
        if player.get_seeking() {
            guard.pending_seek = None;
            progress_pinned = true;
        } else if let Some(pending) = guard.pending_seek.as_ref() {
            let confirmed = playback
                .position
                .as_millis()
                .abs_diff(u128::from(pending.target_ms))
                <= u128::from(SEEK_CONFIRM_TOLERANCE_MS);
            if confirmed || pending.deadline <= Instant::now() || pending.mid != track.id.0 {
                guard.pending_seek = None;
            } else {
                progress_pinned = true;
            }
        }
    }
    if !progress_pinned {
        player.set_position_ms(duration_ms_i32(&playback.position));
        player.set_progress(progress_of(playback));
    }
    player.set_title(track.title.as_str().into());
    player.set_artists(track.artist_names().into());
    player.set_cover(cover_for_track(&track.id.0, track.cover.as_ref()));
    // QQ 远程封面：UI 禁直连 HTTP → 经 daemon CoverGet 换本地产物
    // （先程序化占位，回包后按 mid 复核防串台；每 mid 每进程只请求一次，
    // daemon 侧 covers/ 目录按内容哈希持久去重）。
    if let Some(cover) = track.cover.as_ref() {
        if cover.url.starts_with("https://") {
            spawn_cover_fetch(ui_weak, runtime, track.id.0.clone(), cover.url.clone());
        }
    }
    let quality = playback.actual_quality.as_ref();
    player.set_track_quality(quality.map(quality_label).unwrap_or_default().into());
    player.set_track_max_tier(quality.map(quality_tier).unwrap_or(0));
}

/// 异步取 QQ 封面本地产物（CoverGet）；完成时当前曲仍是发起曲才应用。
fn spawn_cover_fetch(
    ui_weak: &Weak<AppWindow>,
    runtime: &Arc<BackendRuntime>,
    mid: String,
    url: String,
) {
    // 去重：状态推送 ~10Hz，同一 mid 只发起一次（失败也不再重试，下一次
    // 换曲回来时自然重试）。
    thread_local! {
        static REQUESTED: RefCell<std::collections::HashSet<String>> =
            RefCell::new(std::collections::HashSet::new());
    }
    let first_request = REQUESTED.with(|set| set.borrow_mut().insert(mid.clone()));
    if !first_request {
        return;
    }
    let ui_weak = ui_weak.clone();
    let runtime = Arc::clone(runtime);
    runtime.spawn(async move {
        let Ok(Response::Cover(uri)) = crate::backend::request(Request::CoverGet { url }).await
        else {
            return;
        };
        let path = uri.strip_prefix("file://").unwrap_or(&uri).to_string();
        let ui_weak = ui_weak.clone();
        let mid = mid.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let player = Player::get(&ui);
            if player.get_current_mid() != mid.as_str() {
                return; // 换曲竞态：迟到的封面不得串台
            }
            if let Some(image) = load_cover_cached(&path) {
                player.set_cover(image);
            }
        });
    });
}

/// 无当前曲（含 daemon 推送空态）：曲目区归零。
fn clear_now_playing(player: &Player) {
    player.set_has_track(false);
    player.set_current_mid("".into());
    player.set_position_ms(0);
    player.set_duration_ms(0);
    player.set_progress(0.0);
    player.set_title("".into());
    player.set_artists("".into());
    player.set_cover(slint::Image::default());
    player.set_track_quality("".into());
    player.set_track_max_tier(0);
}

/// 当前曲 mid：队列摘要 `current`（规范下标）指向最近一次投影列表
/// （revision 与投影同步推进，见 backend；投影在途的窗口内短暂为空）。
fn current_mid(state: &DaemonState, queue_mids: &[String]) -> String {
    state
        .queue
        .current
        .and_then(|index| queue_mids.get(index))
        .cloned()
        .unwrap_or_default()
}

/// Duration → 毫秒 i32（UI 契约为 int；超过 i32 的时长不存在，直接截断）。
fn duration_ms_i32(d: &Duration) -> i32 {
    d.as_millis() as i32
}

/// 播放进度 0..1（时长未知/为 0 → 0；越界钳制）。
fn progress_of(playback: &PlaybackState) -> f32 {
    let Some(duration) = playback.duration else {
        return 0.0;
    };
    let duration = duration.as_millis() as f32;
    if duration <= 0.0 {
        return 0.0;
    }
    (playback.position.as_millis() as f32 / duration).clamp(0.0, 1.0)
}

/// 音质展示文案：诚实档位名、不带采样率（actual_quality 是本次解析实况）。
fn quality_label(quality: &AudioQuality) -> String {
    match quality {
        AudioQuality::Master => "Master".into(),
        AudioQuality::Atmos => "Atmos".into(),
        AudioQuality::HiRes => "Hi-Res".into(),
        AudioQuality::Flac => "FLAC".into(),
        AudioQuality::Mp3_320 => "320kbps MP3".into(),
        AudioQuality::Mp3_128 => "128kbps MP3".into(),
        AudioQuality::Aac => "AAC".into(),
        AudioQuality::Unknown(raw) => raw.clone(),
    }
}

/// 曲目最高音质档（Quality.effective 的输入；0=标准 1=高清 2=无损 3=Hi-Res）。
fn quality_tier(quality: &AudioQuality) -> i32 {
    match quality {
        AudioQuality::Master | AudioQuality::Atmos | AudioQuality::HiRes => 3,
        AudioQuality::Flac => 2,
        AudioQuality::Mp3_320 => 1,
        _ => 0,
    }
}

/// 队列投影行 → TrackRow（UI 线程：slint::Image 只能在此构造）。
/// 歌手/专辑 mid 在 IPC ID 投影里不存在（链接型列在队列抽屉未使用）。
fn row_from_meta(meta: &QueueRowMeta) -> TrackRow {
    TrackRow {
        mid: meta.mid.as_str().into(),
        source: if hmp_core::TrackProvider::from_id(&meta.mid) == hmp_core::TrackProvider::Local {
            1
        } else {
            0
        },
        title: meta.title.as_str().into(),
        artists: meta.artists.as_str().into(),
        artist_mid: "".into(),
        album: meta.album.as_str().into(),
        album_mid: "".into(),
        duration_ms: meta.duration_ms,
        quality: "".into(),
        cover: queue_cover(meta),
    }
}

/// 队列行封面：本地库 file:// 封面直接读盘（扩列投影带出）；QQ 远程 URL
/// 程序化占位（列表行不做逐行网络取图，仅当前曲经 CoverGet 换真图）。
fn queue_cover(meta: &QueueRowMeta) -> slint::Image {
    if let Some(uri) = &meta.cover_uri {
        if !uri.starts_with("http://") && !uri.starts_with("https://") {
            let path = uri.strip_prefix("file://").unwrap_or(uri);
            if let Some(image) = load_cover_cached(path) {
                return image;
            }
        }
    }
    crate::covers::cover_image(&cover_seed(meta))
}

/// 程序化封面种子：按专辑聚合（与 mock `album:{album-mid}` 同观感），
/// 无专辑信息回退按曲目（同曲恒同图，covers.rs 确定性保证）。
fn cover_seed(meta: &QueueRowMeta) -> String {
    if meta.album.is_empty() {
        format!("album:{}", meta.mid)
    } else {
        format!("album:{}", meta.album)
    }
}

/// 当前曲封面：本地路径 / `file://` → 磁盘图（按 URL 缓存）；
/// http(s)（UI 禁 HTTP 为项目原则，封面代理缺失）→ 按 mid 程序化占位；
/// 无封面 → 程序化占位。
fn cover_for_track(mid: &str, cover: Option<&hmp_core::CoverRef>) -> slint::Image {
    if let Some(url) = cover.map(|c| c.url.as_str()) {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            let path = url.strip_prefix("file://").unwrap_or(url);
            if let Some(image) = load_cover_cached(path) {
                return image;
            }
        }
    }
    crate::covers::cover_image(&format!("album:{mid}"))
}

/// 磁盘封面按路径缓存（thread_local：slint::Image 非 Send，且本函数只在
/// UI 线程被调用；与 covers.rs 的缓存策略一致）。
fn load_cover_cached(path: &str) -> Option<slint::Image> {
    thread_local! {
        static CACHE: RefCell<HashMap<String, slint::Image>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        if let Some(image) = cache.borrow().get(path) {
            return Some(image.clone());
        }
        let image = slint::Image::load_from_path(std::path::Path::new(path)).ok()?;
        cache.borrow_mut().insert(path.to_owned(), image.clone());
        Some(image)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playback_with(duration_ms: u64, position_ms: u64) -> PlaybackState {
        PlaybackState {
            duration: Some(Duration::from_millis(duration_ms)),
            position: Duration::from_millis(position_ms),
            ..PlaybackState::default()
        }
    }

    #[test]
    fn quality_labels_are_honest_tier_names() {
        assert_eq!(quality_label(&AudioQuality::Master), "Master");
        assert_eq!(quality_label(&AudioQuality::Atmos), "Atmos");
        assert_eq!(quality_label(&AudioQuality::HiRes), "Hi-Res");
        assert_eq!(quality_label(&AudioQuality::Flac), "FLAC");
        assert_eq!(quality_label(&AudioQuality::Mp3_320), "320kbps MP3");
        assert_eq!(quality_label(&AudioQuality::Mp3_128), "128kbps MP3");
        assert_eq!(quality_label(&AudioQuality::Aac), "AAC");
        assert_eq!(
            quality_label(&AudioQuality::Unknown("HiRes96".into())),
            "HiRes96"
        );
    }

    #[test]
    fn quality_tiers_match_badge_convention() {
        assert_eq!(quality_tier(&AudioQuality::Master), 3);
        assert_eq!(quality_tier(&AudioQuality::Atmos), 3);
        assert_eq!(quality_tier(&AudioQuality::HiRes), 3);
        assert_eq!(quality_tier(&AudioQuality::Flac), 2);
        assert_eq!(quality_tier(&AudioQuality::Mp3_320), 1);
        assert_eq!(quality_tier(&AudioQuality::Mp3_128), 0);
        assert_eq!(quality_tier(&AudioQuality::Aac), 0);
        assert_eq!(quality_tier(&AudioQuality::Unknown("x".into())), 0);
    }

    #[test]
    fn progress_clamps_and_handles_unknown_duration() {
        assert_eq!(progress_of(&playback_with(200_000, 50_000)), 0.25);
        // 位置越界（seek 竞态窗口）钳制到 1。
        assert_eq!(progress_of(&playback_with(200_000, 500_000)), 1.0);
        // 时长未知/为 0 → 0。
        assert_eq!(progress_of(&PlaybackState::default()), 0.0);
        assert_eq!(progress_of(&playback_with(0, 0)), 0.0);
    }

    #[test]
    fn duration_ms_truncates_to_i32() {
        assert_eq!(duration_ms_i32(&Duration::from_millis(215_123)), 215_123);
        assert_eq!(duration_ms_i32(&Duration::ZERO), 0);
    }

    #[test]
    fn current_mid_follows_queue_summary_index() {
        let mut state = DaemonState::default();
        assert_eq!(current_mid(&state, &[]), "");
        state.queue.current = Some(1);
        assert_eq!(current_mid(&state, &["a".into(), "b".into()]), "b");
        // 投影在途（列表短于摘要下标）→ 空串，不高亮错行。
        assert_eq!(current_mid(&state, &["a".into()]), "");
    }

    #[test]
    fn play_requests_route_by_source() {
        // QQ 曲目 → Track；本地 → Local（`local:` 前缀独立可判）。
        assert_eq!(
            play_request_for_source("003Z3i2C", false),
            PlayRequest::Track(TrackId::new("003Z3i2C"))
        );
        assert_eq!(
            play_request_for_source("local:/music/a.flac", true),
            PlayRequest::Local(TrackId::new("local:/music/a.flac"))
        );
    }

    #[test]
    fn cover_falls_back_to_programmatic_when_no_file() {
        // 无封面 → 程序化占位（确定性渲染，非空图）。
        let image = cover_for_track("mid-1", None);
        assert!(image.size().width > 0);
        // http 封面被禁（UI 禁 HTTP 原则）→ 同样程序化占位。
        let remote = hmp_core::CoverRef {
            url: "https://example.com/a.jpg".into(),
        };
        assert!(cover_for_track("mid-1", Some(&remote)).size().width > 0);
    }

    #[test]
    fn row_from_meta_maps_source_and_fallbacks() {
        let local = QueueRowMeta {
            mid: "local:/music/a.flac".into(),
            title: "本地曲".into(),
            artists: "佚名".into(),
            album: "".into(),
            duration_ms: 1_000,
            cover_uri: None,
        };
        let row = row_from_meta(&local);
        assert_eq!(row.source, 1);
        assert_eq!(row.mid, "local:/music/a.flac");
        assert_eq!(row.duration_ms, 1_000);

        let qq = QueueRowMeta {
            mid: "003Z3i2C".into(),
            title: "003Z3i2C".into(), // 投影回退：标题 = id
            artists: "".into(),
            album: "".into(),
            duration_ms: 0,
            cover_uri: None,
        };
        let row = row_from_meta(&qq);
        assert_eq!(row.source, 0);
        assert_eq!(row.title, "003Z3i2C");
        assert_eq!(row.quality, "", "音质未知 → 徽章留空");
        assert!(row.cover.size().width > 0, "封面回退程序化占位");
    }
}
