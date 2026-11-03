//! 右键菜单动作桥（TrackMenu global ↔ daemon IPC）：
//! - 启停开关：UI 权威值回写 + Prefs 持久化（设置→常规 Switch）
//! - 下一首播放 / 加入队列：`PlayNext` / `QueueAppend` 单曲命令（发送前登记
//!   显示元数据，库外曲目入队后队列行标题不回退成 mid）
//! - 加入播放列表 / 从歌单移除：`PlaylistWrite`（桌面端首个消费方）；受理
//!   （Ok）后触发库刷新，歌单详情页/侧栏模型即时落账——与 daemon
//!   `LibraryChanged` 推送互补，推送丢失或离线窗口期也不落空
//!
//! 错误反馈统一走 [`player_bridge`] 派发路径（未登录等人话文案复用）。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global};

use hmp_core::ipc::Request;
use hmp_core::{PlayRequest, PlaylistWriteOp, TrackId};

use crate::backend::BackendRuntime;
use crate::{AppWindow, Nav, Route, TrackMenu, bridge, player_bridge, prefs};

pub fn bind(ui: &AppWindow, prefs: Arc<Mutex<prefs::Prefs>>, runtime: Arc<BackendRuntime>) {
    let menu = TrackMenu::get(ui);

    {
        // 启停开关：UI 权威值回写 + 持久化（Theme.on_set_mode 同模式）
        let prefs = Arc::clone(&prefs);
        let ui_weak = ui.as_weak();
        menu.on_set_enabled(move |enabled| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            TrackMenu::get(&ui).set_enabled(enabled);
            prefs.lock().expect("prefs").context_menu = enabled;
            prefs::store(&prefs.lock().expect("prefs").clone());
        });
    }

    {
        // 下一首播放：插到当前曲之后（daemon PlayNext 语义）
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui.as_weak();
        menu.on_play_next(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let track = TrackMenu::get(&ui).get_track();
            let mid = track.mid.to_string();
            if mid.is_empty() {
                return;
            }
            player_bridge::remember_meta(
                &mid,
                &track.title,
                &track.artists,
                &track.album,
                track.duration_ms,
            );
            player_bridge::dispatch_command(
                &runtime,
                &ui_weak,
                Request::PlayNext(PlayRequest::Track(TrackId::new(mid))),
            );
        });
    }

    {
        // 加入队列：追加队尾不播（QueueAppend 语义）
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui.as_weak();
        menu.on_append_queue(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let track = TrackMenu::get(&ui).get_track();
            let mid = track.mid.to_string();
            if mid.is_empty() {
                return;
            }
            player_bridge::remember_meta(
                &mid,
                &track.title,
                &track.artists,
                &track.album,
                track.duration_ms,
            );
            player_bridge::dispatch_command(
                &runtime,
                &ui_weak,
                Request::QueueAppend(PlayRequest::Track(TrackId::new(mid))),
            );
        });
    }

    {
        // 加入播放列表：参数 = 自建歌单 id（十进制串）。曲源兼容性已由面板
        // 按 relation 预禁用（owned 拒本地曲），daemon 拒绝仍兜底反馈条
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui.as_weak();
        menu.on_add_to_playlist(move |playlist_id| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(id) = playlist_id.parse::<i64>() else {
                return;
            };
            let track = TrackMenu::get(&ui).get_track();
            if track.mid.is_empty() {
                return;
            }
            let source = if track.source == 1 { "local" } else { "qq" };
            let refresh_runtime = Arc::clone(&runtime);
            player_bridge::dispatch_command_on_ok(
                &runtime,
                &ui_weak,
                Request::PlaylistWrite {
                    op: PlaylistWriteOp::AddTrack {
                        id,
                        source: source.to_string(),
                        key: track.mid.to_string(),
                        title: track.title.to_string(),
                    },
                },
                move |ui| bridge::schedule_refresh(ui, &refresh_runtime),
            );
        });
    }

    {
        // 从歌单移除：歌单 id 取 Nav.param（仅歌单详情页有效，param 为非
        // 数字时 parse 失败即忽略），position = 快照下标（详情页行序即
        // playlist_tracks position 序，library_view 投影保证）
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui.as_weak();
        menu.on_remove_from_playlist(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let nav = Nav::get(&ui);
            if nav.get_route() != Route::Playlist {
                return;
            }
            let Ok(id) = nav.get_param().parse::<i64>() else {
                return;
            };
            let index = TrackMenu::get(&ui).get_index();
            if index < 0 {
                return;
            }
            let refresh_runtime = Arc::clone(&runtime);
            player_bridge::dispatch_command_on_ok(
                &runtime,
                &ui_weak,
                Request::PlaylistWrite {
                    op: PlaylistWriteOp::RemoveTrack {
                        id,
                        position: index as i64,
                    },
                },
                move |ui| bridge::schedule_refresh(ui, &refresh_runtime),
            );
        });
    }
}
