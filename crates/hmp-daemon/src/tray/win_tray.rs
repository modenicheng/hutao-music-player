//! Windows 原生托盘（tray-icon + muda；feature `tray`）。
//!
//! Windows 没有 D-Bus session bus，ksni 路径不可用；这里用 tray-icon
//! （Win32 `Shell_NotifyIcon` + muda 菜单）。约束：图标与菜单事件都经
//! Win32 消息送达，必须在创建托盘的同一线程持续泵消息——本模块起专用
//! 线程跑「`MsgWaitForMultipleObjectsEx` + `PeekMessage` 排空」混合循环，
//! 250ms 唤醒上限同时兼顾控制通道（状态更新/关停）的及时性。
//!
//! 生命周期与 `hmp-smtc` 同款：ready 通道探测启动结果（无 explorer/受
//! 限制会话 → 返回 None 仅 warn）；[`Tray`] 守卫 Drop 时发 Stop 并 join
//! 线程，图标随 `TrayIcon` drop 移除。

use std::sync::mpsc::{self, Receiver, Sender};

use hmp_core::{DaemonState, PlaybackStatus, PlayerCommand, Request};
use tokio::sync::mpsc as tokio_mpsc;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage, WM_QUIT,
};

use crate::engine::EngineHandle;

/// 托盘线程控制消息（tokio 订阅侧 → Win32 线程）。
enum Ctrl {
    /// 状态投影（播放标志 + tooltip 文案，订阅侧已去重）。
    Update(TraySnapshot),
    Stop,
}

/// 跨线程状态投影：Win32 线程只消费这两个字段，不触碰 DaemonState。
#[derive(Clone, Debug, Default, PartialEq)]
struct TraySnapshot {
    playing: bool,
    tooltip: String,
}

/// 菜单项 id（与 `muda::MenuEvent` 比对）。
const ID_TOGGLE: &str = "toggle";
const ID_PREV: &str = "prev";
const ID_NEXT: &str = "next";
const ID_STOP: &str = "stop";
const ID_QUIT: &str = "quit";

/// 托盘守卫：Drop 时停订阅任务、通知 Win32 线程退出并回收（图标随之移除）。
pub struct Tray {
    ctrl_tx: Sender<Ctrl>,
    forward_task: tokio::task::JoinHandle<()>,
    owner_thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.forward_task.abort();
        let _ = self.ctrl_tx.send(Ctrl::Stop);
        if let Some(thread) = self.owner_thread.take() {
            let _ = thread.join();
        }
    }
}

/// 启动托盘（创建失败/无交互会话时返回 None，仅 warn 不 panic）。
pub fn spawn_tray(engine: &EngineHandle) -> Option<Tray> {
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<Ctrl>();
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let command_tx = engine.command_tx.clone();
    let owner_thread = std::thread::Builder::new()
        .name("hmp-tray".into())
        .spawn(move || run_owner(ctrl_rx, ready_tx, command_tx))
        .map_err(|error| {
            tracing::warn!(%error, "failed to spawn tray thread; skipping tray");
            error.to_string()
        })
        .ok()?;
    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, "failed to create tray icon (headless session?); skipping tray");
            let _ = owner_thread.join();
            return None;
        }
        Err(_) => {
            tracing::warn!("tray thread exited before startup; skipping tray");
            let _ = owner_thread.join();
            return None;
        }
    }

    // 状态订阅（tokio 侧）：DaemonState → 投影去重 → Win32 线程。
    // 仅在启动成功后 spawn，失败路径不留孤儿任务。
    let mut state_rx = engine.state_rx.clone();
    let forward_tx = ctrl_tx.clone();
    let forward_task = tokio::spawn(async move {
        loop {
            let snapshot = project(&state_rx.borrow().clone());
            if forward_tx.send(Ctrl::Update(snapshot)).is_err() {
                break;
            }
            if state_rx.changed().await.is_err() {
                break;
            }
        }
    });

    Some(Tray {
        ctrl_tx,
        forward_task,
        owner_thread: Some(owner_thread),
    })
}

/// 托盘属主线程：建菜单/图标 → 回报 ready → 混合消息循环直到 Stop。
fn run_owner(
    ctrl_rx: Receiver<Ctrl>,
    ready_tx: mpsc::SyncSender<Result<(), String>>,
    command_tx: tokio_mpsc::UnboundedSender<Request>,
) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{TrayIconBuilder, TrayIconEvent};

    let menu = Menu::new();
    let toggle_item = MenuItem::with_id(ID_TOGGLE, "Play", true, None);
    let prev_item = MenuItem::with_id(ID_PREV, "Previous", true, None);
    let next_item = MenuItem::with_id(ID_NEXT, "Next", true, None);
    let stop_item = MenuItem::with_id(ID_STOP, "Stop", true, None);
    let quit_item = MenuItem::with_id(ID_QUIT, "Quit", true, None);
    for item in [&toggle_item, &prev_item, &next_item, &stop_item, &quit_item] {
        let _ = menu.append(item);
    }

    let icon_paused = match render_icon(true) {
        Ok(icon) => icon,
        Err(error) => {
            let _ = ready_tx.send(Err(error));
            return;
        }
    };
    let icon_playing = match render_icon(false) {
        Ok(icon) => icon,
        Err(error) => {
            let _ = ready_tx.send(Err(error));
            return;
        }
    };

    // 左键单击留给 TogglePlay，菜单只挂右键。
    let tray = match TrayIconBuilder::new()
        .with_id("hmp")
        .with_tooltip("Hutao Music Player")
        .with_icon(icon_paused.clone())
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
    {
        Ok(tray) => tray,
        Err(error) => {
            let _ = ready_tx.send(Err(error.to_string()));
            return;
        }
    };
    if ready_tx.send(Ok(())).is_err() {
        return; // 守卫已放弃（如 spawn 侧出错），直接退出。
    }

    let mut playing = false;
    // SAFETY: 消息泵只操作本线程的窗口/消息队列；句柄均为 Win32 自管。
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        'run: loop {
            // 1) 控制通道：状态更新 / 关停。
            loop {
                match ctrl_rx.try_recv() {
                    Ok(Ctrl::Update(snapshot)) => {
                        apply_snapshot(
                            &tray,
                            &toggle_item,
                            &icon_playing,
                            &icon_paused,
                            &mut playing,
                            &snapshot,
                        );
                    }
                    Ok(Ctrl::Stop) => break 'run,
                    Err(_) => break,
                }
            }
            // 2) 菜单/托盘事件（crossbeam 队列，非阻塞排空）。
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if let Some(request) = menu_request(&event.id) {
                    let _ = command_tx.send(request);
                }
            }
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if let tray_icon::TrayIconEvent::Click { button, .. } = &event {
                    if *button == tray_icon::MouseButton::Left {
                        let _ = command_tx.send(Request::Command(PlayerCommand::TogglePlay));
                    }
                }
            }
            // 3) 等消息或 250ms（兜底唤醒，保证 Stop/Update 至多半拍延迟）。
            MsgWaitForMultipleObjectsEx(0, std::ptr::null(), 250, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            // 4) 泵排全部待处理消息（MWMO_INPUTAVAILABLE：已就绪消息不阻塞）。
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    break 'run;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    // tray（TrayIcon）在此 drop：同线程移除通知区图标。
}

/// 菜单 id → 引擎请求（与 ksni 菜单同语义）。
fn menu_request(id: &tray_icon::menu::MenuId) -> Option<Request> {
    match id.as_ref() {
        ID_TOGGLE => Some(Request::Command(PlayerCommand::TogglePlay)),
        ID_PREV => Some(Request::Command(PlayerCommand::Previous)),
        ID_NEXT => Some(Request::Command(PlayerCommand::Next)),
        ID_STOP => Some(Request::Command(PlayerCommand::Stop)),
        ID_QUIT => Some(Request::Quit),
        _ => None,
    }
}

/// 状态投影落到托盘：图标/菜单标签（播放态翻转时）+ tooltip（每次）。
fn apply_snapshot(
    tray: &tray_icon::TrayIcon,
    toggle_item: &tray_icon::menu::MenuItem,
    icon_playing: &tray_icon::Icon,
    icon_paused: &tray_icon::Icon,
    playing: &mut bool,
    snapshot: &TraySnapshot,
) {
    if snapshot.playing != *playing {
        *playing = snapshot.playing;
        toggle_item.set_text(if *playing { "Pause" } else { "Play" });
        let icon = if *playing {
            icon_playing.clone()
        } else {
            icon_paused.clone()
        };
        if let Err(error) = tray.set_icon(Some(icon)) {
            tracing::warn!(%error, "failed to update tray icon");
        }
    }
    if let Err(error) = tray.set_tooltip(Some(&snapshot.tooltip)) {
        tracing::warn!(%error, "failed to update tray tooltip");
    }
}

/// DaemonState → 跨线程投影（tooltip 截 120 字符：Win32 szTip 上限 128）。
fn project(state: &DaemonState) -> TraySnapshot {
    let playing = matches!(state.playback.status, PlaybackStatus::Playing);
    let tooltip = match &state.playback.current {
        Some(track) => {
            let artists = track
                .artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let label = if artists.is_empty() {
                track.title.clone()
            } else {
                format!("{} - {}", track.title, artists)
            };
            let verb = if playing { "Playing" } else { "Paused" };
            format!("Hutao Music Player - {verb}: {label}")
        }
        None => "Hutao Music Player".to_string(),
    };
    let tooltip = tooltip.chars().take(120).collect();
    TraySnapshot { playing, tooltip }
}

/// 目标图标边长（通知区标准小图标尺寸）。
const ICON_SIZE: u32 = 32;
/// 每轴超采样倍数：128×128 光栅化后盒滤波降采样到 32×32（近似抗锯齿）。
const SUPERSAMPLE: u32 = 4;
/// 光栅化坐标系边长。
const RASTER: u32 = ICON_SIZE * SUPERSAMPLE;
/// 徽标底色（theme.slint walnut-500 亮色主色 #934A3B）。
const BADGE_RGB: [u8; 3] = [0x93, 0x4A, 0x3B];

/// 程序化生成托盘图标：胡桃色圆角徽标 + 白色双八分音符（播放）或暂停条。
fn render_icon(paused: bool) -> Result<tray_icon::Icon, String> {
    let rgba = render_icon_rgba(paused);
    tray_icon::Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE).map_err(|error| error.to_string())
}

/// 图标像素生成（RGBA 行主序；独立于 `tray_icon::Icon` 以便测试断言）。
fn render_icon_rgba(paused: bool) -> Vec<u8> {
    let raster = RASTER as f32;
    let half = raster / 2.0 - 4.0;
    let radius = 28.0;
    let mut rgba = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let mut sum = [0f32; 4]; // r, g, b, a 累加
            for dy in 0..SUPERSAMPLE {
                for dx in 0..SUPERSAMPLE {
                    let px = (x * SUPERSAMPLE + dx) as f32 + 0.5;
                    let py = (y * SUPERSAMPLE + dy) as f32 + 0.5;
                    let coverage = badge_coverage(px, py, raster / 2.0, half, radius);
                    if coverage > 0.0 {
                        let color = if glyph_inside(px, py, paused) {
                            [255, 255, 255]
                        } else {
                            BADGE_RGB
                        };
                        sum[0] += color[0] as f32 * coverage;
                        sum[1] += color[1] as f32 * coverage;
                        sum[2] += color[2] as f32 * coverage;
                        sum[3] += 255.0 * coverage;
                    }
                }
            }
            let samples = (SUPERSAMPLE * SUPERSAMPLE) as f32;
            let base = ((y * ICON_SIZE + x) * 4) as usize;
            rgba[base] = (sum[0] / samples).round() as u8;
            rgba[base + 1] = (sum[1] / samples).round() as u8;
            rgba[base + 2] = (sum[2] / samples).round() as u8;
            rgba[base + 3] = (sum[3] / samples).round() as u8;
        }
    }
    rgba
}

/// 圆角方形覆盖度（SDF：中心 c、半边长 half、圆角 radius）。
fn badge_coverage(px: f32, py: f32, c: f32, half: f32, radius: f32) -> f32 {
    let qx = ((px - c).abs() - half + radius).max(0.0);
    let qy = ((py - c).abs() - half + radius).max(0.0);
    let sd = (qx * qx + qy * qy).sqrt() - radius;
    (0.5 - sd).clamp(0.0, 1.0)
}

fn in_ellipse(px: f32, py: f32, cx: f32, cy: f32, rx: f32, ry: f32) -> bool {
    let dx = (px - cx) / rx;
    let dy = (py - cy) / ry;
    dx * dx + dy * dy <= 1.0
}

/// 白色符号命中测试（128×128 坐标系；播放 = 双八分音符，暂停 = 双竖条，
/// 两态符号整体都以中心 64 居中）。
fn glyph_inside(px: f32, py: f32, paused: bool) -> bool {
    if paused {
        return (39.0..=59.0).contains(&px) && (40.0..=88.0).contains(&py)
            || (70.0..=90.0).contains(&px) && (40.0..=88.0).contains(&py);
    }
    let beam = (46.0..=94.0).contains(&px) && (30.0..=38.0).contains(&py);
    let stem_left = (46.0..=53.0).contains(&px) && (34.0..=80.0).contains(&py);
    let stem_right = (87.0..=94.0).contains(&px) && (34.0..=72.0).contains(&py);
    let head_left = in_ellipse(px, py, 43.0, 80.0, 11.0, 8.5);
    let head_right = in_ellipse(px, py, 84.0, 72.0, 11.0, 8.5);
    beam || stem_left || stem_right || head_left || head_right
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 播放态图标中心为音符留白（徽标色）；暂停态左条中心为白色。
    #[test]
    fn icon_center_differs_between_states() {
        let playing = render_icon_rgba(false);
        let paused = render_icon_rgba(true);
        assert_eq!(playing.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        let at = |x: u32, y: u32| ((y * ICON_SIZE + x) * 4) as usize;
        // 播放态中心 (16,16)：双音符之间的徽标底色。
        assert_eq!(&playing[at(16, 16)..at(16, 16) + 3], &BADGE_RGB[..]);
        // 暂停态左条内部 (12,16)：白色；中心 (16,16) 为两条间隙 → 徽标底色。
        assert_eq!(&paused[at(12, 16)..at(12, 16) + 3], &[255, 255, 255][..]);
        assert_eq!(&paused[at(16, 16)..at(16, 16) + 3], &BADGE_RGB[..]);
    }

    /// 角落在徽标圆角外：完全透明。
    #[test]
    fn icon_corner_is_transparent() {
        let icon = render_icon_rgba(false);
        assert_eq!(icon.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        assert_eq!(icon[3], 0);
        assert_eq!(icon[(ICON_SIZE * 4) as usize + 3], 0);
    }

    /// 菜单 id → 请求映射须覆盖全部五项（与 ksni 菜单同语义）。
    #[test]
    fn menu_ids_map_to_requests() {
        use tray_icon::menu::MenuId;
        assert!(matches!(
            menu_request(&MenuId::new(ID_TOGGLE)),
            Some(Request::Command(PlayerCommand::TogglePlay))
        ));
        assert!(matches!(
            menu_request(&MenuId::new(ID_PREV)),
            Some(Request::Command(PlayerCommand::Previous))
        ));
        assert!(matches!(
            menu_request(&MenuId::new(ID_NEXT)),
            Some(Request::Command(PlayerCommand::Next))
        ));
        assert!(matches!(
            menu_request(&MenuId::new(ID_STOP)),
            Some(Request::Command(PlayerCommand::Stop))
        ));
        assert!(matches!(
            menu_request(&MenuId::new(ID_QUIT)),
            Some(Request::Quit)
        ));
        assert!(menu_request(&MenuId::new("unknown")).is_none());
    }

    /// tooltip：无曲目 → 基础文案；有曲目 → 带动词与歌手；超长截断。
    #[test]
    fn tooltip_projection() {
        let mut state = DaemonState::default();
        assert_eq!(project(&state).tooltip, "Hutao Music Player");
        state.playback.current = Some(hmp_core::Track::new(hmp_core::TrackId::new("t1"), "Song"));
        state.playback.status = PlaybackStatus::Playing;
        let snapshot = project(&state);
        assert_eq!(snapshot.tooltip, "Hutao Music Player - Playing: Song");
        assert!(snapshot.playing);
        state.playback.current.as_mut().unwrap().title = "很".repeat(200);
        assert!(project(&state).tooltip.chars().count() <= 120);
    }
}
