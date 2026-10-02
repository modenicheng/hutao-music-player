//! Player global ↔ daemon 状态桥（M8，替换 M0-M3 的模拟播放桥）：
//! `DaemonState` → Player 单向映射 + 命令回调 → `Request` 短连接。
//! daemon 是唯一播放后端（docs/PROJECT.md §8.6）：本模块不做任何本地推算
//! （进度/队列/能力全部来自推送），无 daemon（离线降级）时 Player 全空、
//! 命令 no-op。全部 UI 写发生在 UI 线程（回调与订阅投递闭包内）。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Global, Model, ModelRc, TimerMode, VecModel, Weak};

use hmp_core::ipc::{DaemonState, Request, Response};
use hmp_core::{AudioQuality, LoopMode, PlaybackState, PlaybackStatus, PlayerCommand, TrackId};

use crate::backend::{BackendRuntime, QueueRowMeta, UiStateEvent};
use crate::{AppWindow, CommentRow, LyricRow, NowPlaying, Player, TrackRow};

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
        self.volume_echo_until
            .is_none_or(|until| Instant::now() >= until)
    }
}

/// 反馈条自动消退延时（操作提示停留量级；期间有新消息则重置）。
const FEEDBACK_DISMISS: Duration = Duration::from_millis(4_000);

/// 随 PlayList 发出的显示元数据（UI 线程读写；仅队列行显示层兜底）。
struct SentMeta {
    title: String,
    artists: String,
    album: String,
    duration_ms: i32,
}

// 最近一次 play-tracks 的列表元数据（mid → 行显示数据）。daemon 侧 stub
// 只保证 id（搜索结果等库外 QQ 曲目无库行），队列投影 title 会回退成
// mid——这里按发送时的 UI 行补真名。UI 线程独占（thread_local）。
thread_local! {
    static SENT_META: RefCell<HashMap<String, SentMeta>> = RefCell::new(HashMap::new());
}

// 反馈条宿主 weak + 消退定时器（UI 线程独占；invoke_from_event_loop 落地）。
thread_local! {
    static FEEDBACK_UI: RefCell<Weak<AppWindow>> = RefCell::new(Weak::default());
    static FEEDBACK_TIMER: RefCell<slint::Timer> = RefCell::new(slint::Timer::default());
}

/// 命令派发（命令-查询分离 + 错误浮出）：受理（`Response::Ok`）即静默，
/// 真实结果经状态推送呈现；被拒/传输失败 → 反馈条给用户可读的解释
/// （未登录等用户可自救的错误给人话文案，其余透传 daemon 消息）。
fn dispatch_command(runtime: &Arc<BackendRuntime>, ui_weak: &Weak<AppWindow>, req: Request) {
    let runtime = Arc::clone(runtime);
    let ui_weak = ui_weak.clone();
    runtime.spawn(async move {
        match crate::backend::request(req).await {
            Ok(Response::Ok) => {}
            Ok(Response::Err { code, message }) => {
                show_feedback(&ui_weak, friendly_error(code, &message));
            }
            // 查询型响应不会出现在命令路径；宽容忽略。
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("{e}: 播放命令未送达");
                show_feedback(&ui_weak, "播放服务未连接，操作没有生效".into());
            }
        }
    });
}

/// daemon 拒绝码 → 用户可读文案。
fn friendly_error(code: hmp_core::IpcErrorCode, message: &str) -> String {
    match code {
        hmp_core::IpcErrorCode::NotLoggedIn => {
            "未登录：播放在线曲目需要先在 设置 → 账号 扫码登录".into()
        }
        _ => format!("操作未生效：{message}"),
    }
}

/// 设置反馈条文本并（重）启动 4s 消退定时器。任意线程可调（经
/// invoke_from_event_loop 落到 UI 线程；反馈条属性/Timer 均只属于 UI 线程）。
fn show_feedback(ui_weak: &Weak<AppWindow>, message: String) {
    let ui_weak = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        // 消退定时器闭包稍后经 FEEDBACK_UI 取窗口（此刻须先存好）。
        FEEDBACK_UI.with(|cell| *cell.borrow_mut() = ui_weak);
        let Some(ui) = FEEDBACK_UI.with(|cell| cell.borrow().upgrade()) else {
            return;
        };
        Player::get(&ui).set_feedback(message.into());
        FEEDBACK_TIMER.with(|cell| {
            cell.borrow()
                .start(slint::TimerMode::SingleShot, FEEDBACK_DISMISS, || {
                    if let Some(ui) = FEEDBACK_UI.with(|cell| cell.borrow().upgrade()) {
                        Player::get(&ui).set_feedback("".into());
                    }
                });
        });
    });
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

    // —— 命令回调 → Request（命令-查询分离：受理即静默，拒绝/传输失败经
    // 反馈条浮出——此前 `let _ =` 吞错是「按钮点了没反应」且无解释的主因）———
    let ui_weak: Weak<AppWindow> = ui.as_weak();
    macro_rules! command_callback {
        ($name:ident, $req:expr) => {{
            let runtime = Arc::clone(&runtime);
            let ui_weak = ui_weak.clone();
            player.$name(move || {
                dispatch_command(&runtime, &ui_weak, $req);
            });
        }};
    }
    command_callback!(on_toggle_play, Request::Command(PlayerCommand::TogglePlay));
    command_callback!(on_next, Request::Command(PlayerCommand::Next));
    command_callback!(on_previous, Request::Command(PlayerCommand::Previous));
    command_callback!(on_clear_queue, Request::QueueClear { all: true });
    {
        // 循环模式三态切换：闭包经 ui_weak.upgrade() 读实时档位算下一态
        // （顺序 → 单曲循环 → 列表循环 → 顺序）。展示以 daemon 推送回写
        // （与播放/暂停同路数，不本地乐观态）；离线命令被拒弹反馈条、图标不动。
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui_weak.clone();
        player.on_cycle_loop_mode(move || {
            let current = ui_weak
                .upgrade()
                .map(|ui| Player::get(&ui).get_loop_mode())
                .unwrap_or(0);
            let next = match current {
                1 => LoopMode::List,
                2 => LoopMode::None,
                _ => LoopMode::Track,
            };
            dispatch_command(
                &runtime,
                &ui_weak,
                Request::Command(PlayerCommand::SetLoopMode(next)),
            );
        });
    }
    {
        // 随机播放开关：同样读实时值取反。
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui_weak.clone();
        player.on_toggle_shuffle(move || {
            let shuffle = ui_weak
                .upgrade()
                .map(|ui| Player::get(&ui).get_shuffle())
                .unwrap_or(false);
            dispatch_command(
                &runtime,
                &ui_weak,
                Request::Command(PlayerCommand::SetShuffle(!shuffle)),
            );
        });
    }
    {
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui_weak.clone();
        player.on_remove_at(move |index| {
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            dispatch_command(&runtime, &ui_weak, Request::QueueRemove(index));
        });
    }
    {
        // 进度条松手 seek（拖拽全程纯本地回显，见 player-bar.slint）：立即把
        // 展示进度钉在落点并记录 PendingSeek——daemon 确认到点前旧位置推送
        // 不回写，杜绝回跳闪烁；seek 只发一次，不再有逐移动事件的 IPC 风暴。
        let ui_weak = ui_weak.clone();
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
            push_time_text(&player, position_ms as i32, duration_ms as i32);
            player.set_progress(if duration_ms > 0.0 { percent } else { 0.0 });
            echo.lock().expect("echo").pending_seek = Some(PendingSeek {
                target_ms: position_ms,
                mid: player.get_current_mid().to_string(),
                deadline: Instant::now() + SEEK_PIN_TIMEOUT,
            });
            let request = Request::Command(PlayerCommand::Seek(Duration::from_millis(position_ms)));
            let runtime = Arc::clone(&runtime);
            runtime.spawn(async move {
                // seek 被拒（离线等）不弹反馈条：进度展示有 daemon 推送兜底，
                // 拖动场景弹提示反而干扰（重试覆盖冷启动窗口即可）。
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
            let since_send = last_sent.borrow().map(|t| now.saturating_duration_since(t));
            match since_send {
                Some(elapsed) if elapsed < VOLUME_IPC_INTERVAL => {
                    let callback = Rc::clone(&send_pending);
                    ipc_timer.start(
                        TimerMode::SingleShot,
                        VOLUME_IPC_INTERVAL - elapsed,
                        move || callback(),
                    );
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
        // 播放列表入口：可见列表整表入队、点击行为起播（清队列换源语义，
        // 与 Vue playTracks 一致）——此前只发点击行单曲，daemon 把队列
        // replace 成 1 首，上一曲/下一曲从此永久禁用（审计根因）。
        // 显示元数据随发随记（SENT_META overlay）：搜索结果等库外 QQ 曲目
        // 入队后投影回退 title=mid，用发送时的 UI 行补真名（仅显示层）。
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui_weak.clone();
        player.on_play_tracks(move |tracks, start| {
            let count = tracks.row_count();
            if count == 0 {
                return;
            }
            let start = usize::try_from(start)
                .ok()
                .filter(|index| *index < count)
                .unwrap_or(0)
                .min(count - 1);
            let mut ids = Vec::with_capacity(count);
            let mut sent_meta = HashMap::new();
            // 空 mid 行剔除（如历史页库外残行的投影兜底）——空 id 会让 daemon
            // 解析器整体失败；起播下标按剔除量重映射，保持指向同一曲目。
            let mut play_start = start;
            for index in 0..count {
                let Some(row) = tracks.row_data(index) else {
                    continue;
                };
                let mid = row.mid.to_string();
                if mid.is_empty() {
                    if index < start {
                        play_start = play_start.saturating_sub(1);
                    }
                    continue;
                }
                sent_meta.insert(
                    mid.clone(),
                    SentMeta {
                        title: row.title.to_string(),
                        artists: row.artists.to_string(),
                        album: row.album.to_string(),
                        duration_ms: row.duration_ms,
                    },
                );
                ids.push(TrackId::new(mid));
            }
            if ids.is_empty() {
                return;
            }
            let start = play_start.min(ids.len() - 1);
            SENT_META.with(|cell| *cell.borrow_mut() = sent_meta);
            dispatch_command(&runtime, &ui_weak, Request::PlayList { ids, start });
        });
    }

    {
        // 队列点击：跳到该位置播放（QueuePlayAt，队列不被单曲替换，AUDIT §8.8）。
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui_weak.clone();
        player.on_play_at(move |index| {
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            dispatch_command(&runtime, &ui_weak, Request::QueuePlayAt(index));
        });
    }

    {
        // 歌词行点击：精确时间 seek（Seek 序列化为秒粒度，毫秒取整）。
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui_weak.clone();
        player.on_seek_ms(move |ms| {
            if ms < 0 {
                return;
            }
            dispatch_command(
                &runtime,
                &ui_weak,
                Request::Command(PlayerCommand::Seek(std::time::Duration::from_millis(
                    ms as u64,
                ))),
            );
        });
    }

    {
        // 评论排序切换 / 首次打开：重拉（是否真的重拉由 maybe_load_comments
        // 的 key 去重决定，UI 回调只是触发信号）。
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui_weak.clone();
        NowPlaying::get(ui).on_request_comments(move || {
            maybe_load_comments(&runtime, &ui_weak);
        });
    }

    bind_ui_toggles(ui, &runtime);

    // —— 订阅：DaemonState 推送 → Player global（UI 线程应用）———
    let ui_weak: Weak<AppWindow> = ui.as_weak();
    let runtime_for_state = Arc::clone(&runtime);
    let echo_for_state = Arc::clone(&echo);
    let handler = Arc::new(move |event: UiStateEvent| {
        apply_event(
            &ui_weak,
            &runtime_for_state,
            &queue_mids,
            &echo_for_state,
            event,
        );
    });
    crate::backend::spawn_state_subscription(&runtime, handler);
}

/// SetVolume 命令（短连接；受理即返回，结果经状态推送呈现）。
/// 音量不弹反馈条：拖动场景逐次提示是噪音，推送值即最终真相。
fn send_volume_ipc(runtime: &Arc<BackendRuntime>, volume: f64) {
    runtime.spawn(async move {
        let _ = crate::backend::request(Request::Command(PlayerCommand::SetVolume(volume))).await;
    });
}

/// 队列抽屉 / 播放页 overlay 的纯 UI 回写（不产生 IPC；与模拟桥行为一致）。
fn bind_ui_toggles(ui: &AppWindow, runtime: &Arc<BackendRuntime>) {
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
    let runtime_for_overlay = Arc::clone(runtime);
    player.on_show_overlay(move || {
        if let Some(ui) = ui_weak.upgrade() {
            Player::get(&ui).set_overlay_visible(true);
            // 打开即装载评论（key 去重；歌词走换曲路径，不在此重拉）
            maybe_load_comments(&runtime_for_overlay, &ui_weak);
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

    // 库内容变更（扫描/监听/reconcile/写命令落库）：防抖 + 后台重查 sqlite
    // 刷新库页与当前详情页（AUDIT §8.9；UI 线程只做模型落地，不再同步读库）。
    if event.library_changed {
        crate::bridge::schedule_refresh(&ui, runtime);
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
        apply_offline(&player, ui_weak);
        // 订阅只在在线↔离线翻转时投递一次空态：提示一次，不随重试刷屏。
        show_feedback(ui_weak, "播放服务未连接，播放操作暂不可用".into());
        return;
    };
    let mids = queue_mids.lock().expect("queue mids").clone();
    apply_daemon_state(&player, &state, &mids, echo, ui_weak, runtime);
}

/// 离线（无 daemon / daemon 已退出）：播放态全空（音量保留本地值不重置）。
fn apply_offline(player: &Player, ui_weak: &Weak<AppWindow>) {
    player.set_playing(false);
    player.set_loading(false);
    player.set_can_previous(false);
    player.set_can_next(false);
    player.set_queue_current_mid("".into());
    // 播放模式回到默认（顺序播放、非随机）；重连后由 daemon 推送恢复真值。
    player.set_loop_mode(0);
    player.set_shuffle(false);
    LAST_SHOWN_ERROR.with(|cell| *cell.borrow_mut() = None);
    clear_now_playing(player, ui_weak);
}

// daemon `last_error` 已展示消息（去重：状态推送 ~10Hz 携带同一错误，
// 只在消息变化时弹一次；成功装载清空 last_error 时同步复位）。
// 其余为播放页（M6）数据管线状态：全部仅在 UI 线程触碰。
thread_local! {
    static LAST_SHOWN_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
    /// 歌词时间戳（毫秒升序，与 NowPlaying.lyrics 同源）；active-line 折算用
    static LYRIC_STAMPS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// 上一次状态推送的当前曲 mid（换曲检测：重置喜欢态/重拉歌词与评论）
    static PREV_MID: RefCell<String> = const { RefCell::new(String::new()) };
    /// 已装载评论的 (mid, sort)：未变不重拉（overlay 打开期间按推送驱动）
    static COMMENT_KEY: RefCell<Option<(String, i32)>> = const { RefCell::new(None) };
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
    // 引擎阶段：装载/解析中间态暴露给 UI（主键/播放页的中间态依据）。
    player.set_loading(matches!(
        state.phase,
        hmp_core::EnginePhase::Resolving | hmp_core::EnginePhase::Loading
    ));
    // daemon 侧引擎错误（解析失败/取流失败等）浮出：与命令拒绝同一反馈条。
    match &state.last_error {
        Some(err) => {
            let is_new = LAST_SHOWN_ERROR.with(|cell| {
                let mut shown = cell.borrow_mut();
                let changed = shown.as_deref() != Some(err.message.as_str());
                if changed {
                    *shown = Some(err.message.clone());
                }
                changed
            });
            if is_new {
                show_feedback(ui_weak, format!("播放出错：{}", err.message));
            }
        }
        None => LAST_SHOWN_ERROR.with(|cell| *cell.borrow_mut() = None),
    }
    // 音量展示用用户原值（playback.volume 含 RG 补偿，回设会静默偏移，
    // AUDIT §8.12；引擎恒随状态发布 user_volume，0.0 是合法静音不再是
    // 「未设置」哨兵——旧回退 hack 会让静音后 thumb 回跳补偿值）。
    if echo.lock().expect("echo").volume_echo_expired() {
        player.set_volume(playback.user_volume.clamp(0.0, 1.0) as f32);
    }
    player.set_can_previous(state.caps.can_go_previous);
    player.set_can_next(state.caps.can_go_next);
    player.set_queue_current_mid(current_mid(state, queue_mids).into());
    // 播放模式（循环/随机）跟随 daemon 推送（唯一状态出口；切换命令受理即
    // 静默，新值随下一次推送回显，不做本地乐观态避免与推送打架）。
    player.set_loop_mode(loop_mode_index(playback.loop_mode));
    player.set_shuffle(playback.shuffle);

    let Some(track) = &playback.current else {
        echo.lock().expect("echo").pending_seek = None;
        clear_now_playing(player, ui_weak);
        return;
    };
    player.set_has_track(true);
    player.set_current_mid(track.id.0.as_str().into());
    let duration = playback.duration.as_ref().map(duration_ms_i32).unwrap_or(0);
    player.set_duration_ms(duration);
    push_time_text(player, player.get_position_ms(), duration);

    // 换曲检测：重置播放页 mock 态（喜欢回落）并重拉歌词/评论。
    let mid = track.id.0.clone();
    let mid_changed = PREV_MID.with(|cell| {
        let mut prev = cell.borrow_mut();
        let changed = prev.as_str() != mid;
        if changed {
            *prev = mid.clone();
        }
        changed
    });
    if mid_changed {
        if let Some(ui) = ui_weak.upgrade() {
            let np = NowPlaying::get(&ui);
            np.set_liked(false);
            np.set_active_line(-1);
            LYRIC_STAMPS.with(|cell| cell.borrow_mut().clear());
            // 本地/QQ 同一歌词管线：本地曲 daemon 先读同目录 .lrc/内嵌标签，
            // 缺失再按标题+歌手检索 QQ 兜底（本地优先）；QQ 曲按 mid 直取。
            np.set_lyrics_loading(true);
            spawn_lyric_fetch(
                ui_weak,
                runtime,
                mid.clone(),
                track.title.clone(),
                track.artist_names(),
            );
            COMMENT_KEY.with(|cell| *cell.borrow_mut() = None);
            maybe_load_comments(runtime, ui_weak);
        }
    }

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
        let position = duration_ms_i32(&playback.position);
        player.set_position_ms(position);
        push_time_text(player, position, player.get_duration_ms());
        player.set_progress(progress_of(playback));
        update_active_line(ui_weak, position);
    }
    player.set_title(track.title.as_str().into());
    player.set_artists(track.artist_names().into());
    player.set_album(
        track
            .album
            .as_ref()
            .map(|a| a.name.as_str())
            .unwrap_or_default()
            .into(),
    );
    let cover = cover_for_track(&track.id.0, track.cover.as_ref());
    player.set_cover(cover.clone());
    // 曲目层取色 + 环境层模糊底图随封面更新（apply_cover 内部按键去重，
    // 10Hz 推送同曲零重算）；QQ 远程封面先吃程序化占位的取色，真图回包
    // 后在 spawn_cover_fetch 落点按 mid|url 键重算
    let cover_key = match track.cover.as_ref() {
        Some(cover) => format!("{}|{}", track.id.0, cover.url),
        None => format!("{}|prog", track.id.0),
    };
    crate::track_theme::apply_cover(ui_weak, &cover_key, &cover);
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

/// 播放页歌词装载：LyricGet（daemon 本地优先 + QQ 检索兜底）→ LRC 解析 →
/// 模型落地。每 id 每进程只请求一次（失败不重试，换曲再回来时自然重试）；
/// 回包时当前曲已换 → 丢弃（串台守卫，同封面取回路径）。
fn spawn_lyric_fetch(
    ui_weak: &Weak<AppWindow>,
    runtime: &Arc<BackendRuntime>,
    id: String,
    title: String,
    artist: String,
) {
    thread_local! {
        static REQUESTED: RefCell<std::collections::HashSet<String>> = RefCell::new(HashSet::new());
    }
    if !REQUESTED.with(|set| set.borrow_mut().insert(id.clone())) {
        return;
    }
    let ui_weak = ui_weak.clone();
    let runtime = Arc::clone(runtime);
    runtime.spawn(async move {
        let lines = match crate::backend::request(Request::LyricGet {
            id: id.clone(),
            title,
            artist,
        })
        .await
        {
            Ok(Response::Lyric(page)) => crate::lyrics::parse_lrc(&page.lyric, &page.translation),
            _ => Vec::new(),
        };
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if Player::get(&ui).get_current_mid() != id.as_str() {
                return;
            }
            let np = NowPlaying::get(&ui);
            LYRIC_STAMPS.with(|cell| {
                *cell.borrow_mut() = lines.iter().map(|line| line.timestamp_ms).collect()
            });
            let rows: Vec<LyricRow> = lines
                .into_iter()
                .map(|line| LyricRow {
                    timestamp_ms: line.timestamp_ms as i32,
                    text: line.text.into(),
                    translation: line.translation.into(),
                })
                .collect();
            np.set_lyrics(ModelRc::new(VecModel::from(rows)));
            np.set_lyrics_loading(false);
            np.set_lyrics_generation(np.get_lyrics_generation() + 1);
        });
    });
}

/// 播放位置 → 歌词焦点行下标（末条 ≤ 位置的最后一行；前奏为 -1）。
/// 推送 ~10Hz 驱动，行级高亮足够；拖拽进度条期间（pinned）不折算。
fn update_active_line(ui_weak: &Weak<AppWindow>, position_ms: i32) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    let stamps = LYRIC_STAMPS.with(|cell| cell.borrow().clone());
    let position = position_ms.max(0) as u64;
    let mut line: i32 = -1;
    for (index, stamp) in stamps.iter().enumerate() {
        if *stamp <= position {
            line = index as i32;
        } else {
            break;
        }
    }
    let np = NowPlaying::get(&ui);
    if np.get_active_line() != line {
        np.set_active_line(line);
    }
}

/// 评论装载驱动：overlay 打开 + 有当前曲 + (mid, sort) 未变 → 拉取。
/// 状态推送 ~10Hz 携带调用（key 去重后是幂等空转）；排序切换改 key 后生效。
fn maybe_load_comments(runtime: &Arc<BackendRuntime>, ui_weak: &Weak<AppWindow>) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    let player = Player::get(&ui);
    if !player.get_overlay_visible() {
        return;
    }
    let mid = player.get_current_mid().to_string();
    if mid.is_empty() {
        return;
    }
    let np = NowPlaying::get(&ui);
    let sort = np.get_comment_sort();
    let key = (mid.clone(), sort);
    if COMMENT_KEY.with(|cell| cell.borrow().as_ref() == Some(&key)) {
        return;
    }
    COMMENT_KEY.with(|cell| *cell.borrow_mut() = Some(key));
    if mid.starts_with("local:") {
        // 本地曲目无评论域：直接就绪空态（诚实：不显示加载中）
        np.set_comments(ModelRc::new(VecModel::from(Vec::<CommentRow>::new())));
        np.set_comment_total("".into());
        np.set_comment_state(2);
        return;
    }
    np.set_comment_state(1);
    spawn_comment_fetch(ui_weak, runtime, mid, sort);
}

/// 评论拉取（CommentList）：回包按 (mid, sort) 守卫，只应用仍有效的结果。
fn spawn_comment_fetch(
    ui_weak: &Weak<AppWindow>,
    runtime: &Arc<BackendRuntime>,
    mid: String,
    sort: i32,
) {
    let ui_weak = ui_weak.clone();
    let runtime = Arc::clone(runtime);
    runtime.spawn(async move {
        let sort_name = if sort == 1 { "new" } else { "hot" };
        let result = crate::backend::request(Request::CommentList {
            mid: mid.clone(),
            sort: sort_name.into(),
            page: 1,
            num: 20,
        })
        .await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let np = NowPlaying::get(&ui);
            if Player::get(&ui).get_current_mid() != mid.as_str() || np.get_comment_sort() != sort {
                return;
            }
            match result {
                Ok(Response::CommentList(page)) => {
                    let rows: Vec<CommentRow> = page
                        .comments
                        .into_iter()
                        .map(|c| CommentRow {
                            cm_id: c.cm_id.into(),
                            initial: c
                                .nickname
                                .chars()
                                .next()
                                .map(String::from)
                                .unwrap_or_default()
                                .into(),
                            nickname: c.nickname.into(),
                            content: c.content.into(),
                            time_text: crate::library_view::format_stamp(c.time).into(),
                            like_text: like_count_text(c.like_count).into(),
                        })
                        .collect();
                    np.set_comment_total(like_count_text(page.total).into());
                    np.set_comments(ModelRc::new(VecModel::from(rows)));
                    np.set_comment_state(2);
                }
                _ => np.set_comment_state(3),
            }
        });
    });
}

/// 计数文案：≥1万 → "x.x万"（与原型 formatCount 同口径）。
fn like_count_text(count: i64) -> String {
    if count >= 10_000 {
        format!("{:.1}万", count as f64 / 10_000.0)
    } else {
        count.to_string()
    }
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
        // 取色键与同步路径一致（mid|url）：真图落地后下一帧推送同键零重算
        let cover_key = format!("{mid}|{url}");
        let Ok(Response::Cover(uri)) = crate::backend::request(Request::CoverGet { url }).await
        else {
            return;
        };
        let path = crate::covers::file_uri_to_path(&uri).unwrap_or(uri);
        let ui_weak = ui_weak.clone();
        let mid = mid.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let player = Player::get(&ui);
            let Some(image) = load_cover_cached(&path) else {
                return;
            };
            // 队列抽屉行内原地换图：与当前曲无关——迟到的封面同样更新抽屉行
            // （daemon 已回写 cover_uri，此后队列重建直接读盘，此处补本次会话）
            update_queue_row_cover(&player, &mid, image.clone());
            if player.get_current_mid() != mid.as_str() {
                return; // 换曲竞态：播放条/取色只认当前曲
            }
            player.set_cover(image.clone());
            // 真图取色覆写程序化占位的取色
            crate::track_theme::apply_cover(&ui_weak, &cover_key, &image);
        });
    });
}

/// 队列模型中同 mid 行的封面原地更新（抽屉渲染 TrackRow.cover）。
fn update_queue_row_cover(player: &Player, mid: &str, image: slint::Image) {
    let model = player.get_queue();
    let Some(vec_model) = model.as_any().downcast_ref::<slint::VecModel<TrackRow>>() else {
        return;
    };
    for i in 0..vec_model.iter().count() {
        let mut row = match vec_model.row_data(i) {
            Some(r) => r,
            None => continue,
        };
        if row.mid.as_str() == mid {
            row.cover = image;
            vec_model.set_row_data(i, row);
            return;
        }
    }
}

/// 控制台两端时间标签（DESIGN「两端时间 tabular-nums，剩余以 -m:ss」）：
/// 随位置/时长每次推送同步刷新；无时长（未知）时剩余侧留空。
fn push_time_text(player: &Player, position_ms: i32, duration_ms: i32) {
    player.set_elapsed_text(crate::format::format_duration(position_ms.max(0) as u64).into());
    player.set_remaining_text(
        if duration_ms > 0 {
            format!(
                "-{}",
                crate::format::format_duration((duration_ms - position_ms).max(0) as u64)
            )
        } else {
            String::new()
        }
        .into(),
    );
}

/// 无当前曲（含 daemon 推送空态）：曲目区归零。
fn clear_now_playing(player: &Player, ui_weak: &Weak<AppWindow>) {
    player.set_has_track(false);
    player.set_current_mid("".into());
    player.set_position_ms(0);
    player.set_duration_ms(0);
    push_time_text(player, 0, 0);
    player.set_progress(0.0);
    player.set_title("".into());
    player.set_artists("".into());
    player.set_album("".into());
    player.set_cover(slint::Image::default());
    player.set_track_quality("".into());
    player.set_track_max_tier(0);
    player.set_loading(false);
    // 播放页数据随曲清空（无曲时 overlay 呈空态，不留上一曲残页）
    PREV_MID.with(|cell| cell.borrow_mut().clear());
    LYRIC_STAMPS.with(|cell| cell.borrow_mut().clear());
    COMMENT_KEY.with(|cell| *cell.borrow_mut() = None);
    if let Some(ui) = ui_weak.upgrade() {
        // 无曲 → 曲目层调色整族回落品牌胡桃木、环境层清空
        crate::track_theme::apply_fallback(&ui);
        let np = NowPlaying::get(&ui);
        np.set_liked(false);
        np.set_active_line(-1);
        np.set_lyrics(ModelRc::new(VecModel::from(Vec::<LyricRow>::new())));
        np.set_lyrics_loading(false);
        np.set_lyrics_generation(np.get_lyrics_generation() + 1);
        np.set_comments(ModelRc::new(VecModel::from(Vec::<CommentRow>::new())));
        np.set_comment_state(0);
        np.set_comment_total("".into());
    }
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

/// `hmp_core::LoopMode` → UI 档位（0=顺序播放 1=单曲循环 2=列表循环）。
/// daemon 枚举序是 None/List/Track；UI 三态循环序刻意为 None→Track→List
/// （顺序播放居首，与主流播放器一致），见 stores.slint 的 loop-mode 契约。
fn loop_mode_index(mode: LoopMode) -> i32 {
    match mode {
        LoopMode::None => 0,
        LoopMode::Track => 1,
        LoopMode::List => 2,
    }
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
    let mut row = TrackRow {
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
    };
    // 库外曲目（搜索结果等，daemon stub 只有 id）标题回退 mid 时，用
    // play-tracks 发出时的 UI 行元数据补真名（仅显示层，不写库）。
    if row.title == row.mid {
        SENT_META.with(|cell| {
            if let Some(sent) = cell.borrow().get(row.mid.as_str()) {
                row.title = sent.title.as_str().into();
                row.artists = sent.artists.as_str().into();
                row.album = sent.album.as_str().into();
                if sent.duration_ms > 0 {
                    row.duration_ms = sent.duration_ms;
                }
            }
        });
    }
    row
}

/// 队列行封面：本地库 file:// 封面直接读盘（扩列投影带出；daemon 已把取到
/// 的本地产物回写 cover_uri，播过的 QQ 曲同样命中）；远程 URL 程序化占位
/// （逐行网络取图不做，当前曲取图回包另有原地换图补齐）。
fn queue_cover(meta: &QueueRowMeta) -> slint::Image {
    if let Some(uri) = &meta.cover_uri {
        if !uri.starts_with("http://") && !uri.starts_with("https://") {
            if let Some(path) = crate::covers::file_uri_to_path(uri) {
                if let Some(image) = load_cover_cached(&path) {
                    return image;
                }
            }
        }
    }
    crate::covers::cover_image(&cover_seed(meta))
}

/// 程序化封面种子：按专辑聚合（同专辑恒同图，`album:{album-mid}` 惯例），
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
            // file:// 双形态（persist_cover 宽容形态 `file://C:\...` 与历史
            // 规范形态 `file:///C:/...`）都收：裸剥前缀把规范形态解析成
            // `/C:/...`（Windows 读不到）→ 盘上有图恒占位（§18 库行实锤）。
            if let Some(path) = crate::covers::file_uri_to_path(url) {
                if let Some(image) = load_cover_cached(&path) {
                    return image;
                }
                tracing::debug!(path, "cover file unreadable; fallback to placeholder");
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
    fn loop_mode_index_matches_ui_cycle_order() {
        // UI 三态循环序：0=顺序播放 1=单曲循环 2=列表循环（非 daemon 枚举序）。
        assert_eq!(loop_mode_index(LoopMode::None), 0);
        assert_eq!(loop_mode_index(LoopMode::Track), 1);
        assert_eq!(loop_mode_index(LoopMode::List), 2);
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
    fn friendly_error_gives_login_hint_for_not_logged_in() {
        assert_eq!(
            friendly_error(hmp_core::IpcErrorCode::NotLoggedIn, "not logged in"),
            "未登录：播放在线曲目需要先在 设置 → 账号 扫码登录"
        );
        // 其余错误透传 daemon 消息（用户看得到失败原因）。
        assert_eq!(
            friendly_error(hmp_core::IpcErrorCode::Internal, "boom"),
            "操作未生效：boom"
        );
    }

    #[test]
    fn row_from_meta_overlays_sent_meta_for_out_of_library_tracks() {
        let meta = QueueRowMeta {
            mid: "0039MnYb0qxYhV".into(),
            // 库外曲目投影回退：标题 = id
            title: "0039MnYb0qxYhV".into(),
            artists: "".into(),
            album: "".into(),
            duration_ms: 0,
            cover_uri: None,
        };
        SENT_META.with(|cell| {
            *cell.borrow_mut() = HashMap::from([(
                "0039MnYb0qxYhV".to_string(),
                SentMeta {
                    title: "夜曲".into(),
                    artists: "周杰伦".into(),
                    album: "十一月的萧邦".into(),
                    duration_ms: 226_000,
                },
            )]);
        });
        let row = row_from_meta(&meta);
        assert_eq!(row.title, "夜曲");
        assert_eq!(row.artists, "周杰伦");
        assert_eq!(row.album, "十一月的萧邦");
        assert_eq!(row.duration_ms, 226_000);
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
