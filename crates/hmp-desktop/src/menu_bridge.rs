//! 右键菜单动作桥（TrackMenu global ↔ daemon IPC）：
//! - 启停开关：UI 权威值回写 + Prefs 持久化（设置→常规 Switch）
//! - 下一首播放 / 加入队列：`PlayNext` / `QueueAppend` 单曲命令（发送前登记
//!   显示元数据，库外曲目入队后队列行标题不回退成 mid）
//! - 加入播放列表 / 从歌单移除：`PlaylistWrite`（桌面端首个消费方）；受理
//!   （Ok）后触发库刷新，歌单详情页/侧栏模型即时落账——与 daemon
//!   `LibraryChanged` 推送互补，推送丢失或离线窗口期也不落空
//! - 新建歌单：`PlaylistWriteOp::Create` → `Response::Created(id)` → 刷新库 +
//!   直达新歌单页（侧栏「+」与「我喜欢」页的命名弹层发起）
//! - 歌单级重命名 / 删除：`PlaylistWriteOp::{Rename,Delete}`（歌单页「⋯」与
//!   侧栏歌单行右键菜单发起）。owned 重命名由 daemon 拒绝
//!   （`RemoteRenameUnsupported`，UI 侧先按 relation 禁用 + 注明原因）；
//!   subscribed 删除 = 取消收藏；owned 删除 = 行保留到远端删除成功。
//!   拒绝/未连接一律反馈条人话，不静默失败。
//!
//! 错误反馈统一走 [`player_bridge`] 派发路径（未登录等人话文案复用）。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global};

use hmp_core::ipc::Request;
use hmp_core::{PlayRequest, PlaylistWriteOp, Response, TrackId};

use crate::backend::{BackendError, BackendRuntime};
use crate::{AppWindow, Data, Nav, PlaylistMenu, Route, TrackMenu, bridge, player_bridge, prefs};

/// 歌单名称校验（纯逻辑，单测锚点）：去首尾空白，空名 → None。
fn playlist_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// 新建歌单回包 → 动作（纯逻辑决策表，单测钉行为）。
#[derive(Debug, PartialEq)]
enum CreateOutcome {
    /// 成功：新歌单 id（刷新库 + 直达）
    Created(i64),
    /// 未生效：反馈条文案
    Failed(String),
}

fn create_outcome(result: &Result<Response, BackendError>) -> CreateOutcome {
    match result {
        Ok(Response::Created(id)) => CreateOutcome::Created(*id),
        Ok(Response::Err { message, .. }) => {
            CreateOutcome::Failed(format!("新建歌单失败：{message}"))
        }
        Ok(_) => CreateOutcome::Failed("新建歌单未生效：守护进程应答异常".into()),
        Err(_) => CreateOutcome::Failed("播放服务未连接，歌单没有创建".into()),
    }
}

/// 新建歌单回包应用（UI 线程）：成功 → 刷新库页/侧栏（受理时 daemon 已同步
/// 落库，随后的详情直读能命中）→ 直达新歌单页；失败 → 反馈条。
fn apply_create_reply(
    ui: &AppWindow,
    runtime: &Arc<BackendRuntime>,
    result: Result<Response, BackendError>,
) {
    match create_outcome(&result) {
        CreateOutcome::Created(id) => {
            bridge::schedule_refresh(ui, runtime);
            Nav::get(ui).invoke_navigate(Route::Playlist, id.to_string().into());
        }
        CreateOutcome::Failed(message) => {
            let _ = result;
            player_bridge::show_feedback(&ui.as_weak(), message);
        }
    }
}

/// 歌单级写操作（重命名/删除）回包 → 动作（纯逻辑决策表，单测钉行为）。
#[derive(Debug, PartialEq)]
enum PlaylistWriteOutcome {
    /// 受理：刷新库 + 反馈条文案（空串 = 不提示）
    Accepted(String),
    /// 未生效：反馈条文案
    Failed(String),
}

fn rename_outcome(result: &Result<Response, BackendError>) -> PlaylistWriteOutcome {
    match result {
        Ok(Response::Ok) => PlaylistWriteOutcome::Accepted("歌单已重命名".into()),
        // daemon 对 owned/subscribed 的拒绝也走这里：UI 已按 relation 禁用，
        // 兜底仍把 daemon 的人话原样透出（不静默吞掉）
        Ok(Response::Err { message, .. }) => {
            PlaylistWriteOutcome::Failed(format!("重命名失败：{message}"))
        }
        Ok(_) => PlaylistWriteOutcome::Failed("重命名未生效：守护进程应答异常".into()),
        Err(_) => PlaylistWriteOutcome::Failed("播放服务未连接，歌单没有重命名".into()),
    }
}

/// 删除成功文案按 daemon 语义分流：subscribed = 取消收藏、owned = 先本地挂起
/// 等远端删除（行保留到成功）、local = 立即删除。
fn delete_success_text(relation: &str) -> &'static str {
    match relation {
        "subscribed" => "已取消收藏",
        "owned" => "已提交删除，正在同步到云端",
        _ => "歌单已删除",
    }
}

fn delete_outcome(relation: &str, result: &Result<Response, BackendError>) -> PlaylistWriteOutcome {
    match result {
        Ok(Response::Ok) => PlaylistWriteOutcome::Accepted(delete_success_text(relation).into()),
        Ok(Response::Err { message, .. }) => {
            PlaylistWriteOutcome::Failed(format!("删除歌单失败：{message}"))
        }
        Ok(_) => PlaylistWriteOutcome::Failed("删除未生效：守护进程应答异常".into()),
        Err(_) => PlaylistWriteOutcome::Failed("播放服务未连接，歌单没有删除".into()),
    }
}

/// 重命名/删除受理后：刷新库（侧栏与当前详情页同步）；删除当前正在浏览的
/// 歌单（本地行即刻消失的 local/subscribed）时回「我喜欢」，避免停在
/// 「未找到歌单」。owned 删除先挂起、行保留，页面不动。
fn apply_playlist_write(
    ui: &AppWindow,
    runtime: &Arc<BackendRuntime>,
    outcome: PlaylistWriteOutcome,
    deleted: Option<(i64, &str)>,
) {
    match outcome {
        PlaylistWriteOutcome::Accepted(message) => {
            bridge::schedule_refresh(ui, runtime);
            if let Some((id, relation)) = deleted
                && relation != "owned"
            {
                let nav = Nav::get(ui);
                if nav.get_route() == Route::Playlist && nav.get_param() == id.to_string() {
                    nav.invoke_navigate(Route::Library, "".into());
                }
            }
            if !message.is_empty() {
                player_bridge::show_feedback(&ui.as_weak(), message);
            }
        }
        PlaylistWriteOutcome::Failed(message) => {
            player_bridge::show_feedback(&ui.as_weak(), message);
        }
    }
}

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
            // key 校验（AUDIT §30 U5）：行 mid 即库内 source_key，随序号一并下发；
            // daemon 发现该序号上已不是这一首（快照过期）会拒绝并提示刷新。
            let track = TrackMenu::get(&ui).get_track();
            let expect_key = (!track.mid.is_empty()).then(|| track.mid.to_string());
            let refresh_runtime = Arc::clone(&runtime);
            player_bridge::dispatch_command_on_ok(
                &runtime,
                &ui_weak,
                Request::PlaylistWrite {
                    op: PlaylistWriteOp::RemoveTrack {
                        id,
                        position: index as i64,
                        expect_key,
                    },
                },
                move |ui| bridge::schedule_refresh(ui, &refresh_runtime),
            );
        });
    }

    {
        // 歌单级重命名：菜单「重命名」→ 命名弹层（复用 NewPlaylistForm）→
        // PlaylistWrite::Rename。owned/subscribed 由 UI 预先禁用（daemon 也会
        // 拒绝 RemoteRenameUnsupported——两层都不静默）。
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui.as_weak();
        PlaylistMenu::get(ui).on_rename(move |raw| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Some(name) = playlist_name(&raw) else {
                player_bridge::show_feedback(&ui.as_weak(), "歌单名称不能为空".into());
                return;
            };
            let Ok(id) = PlaylistMenu::get(&ui).get_id().parse::<i64>() else {
                return;
            };
            let rt = Arc::clone(&runtime);
            player_bridge::dispatch_with_reply(
                &runtime,
                &ui_weak,
                Request::PlaylistWrite {
                    op: PlaylistWriteOp::Rename { id, name },
                },
                move |ui, result| {
                    apply_playlist_write(ui, &rt, rename_outcome(&result), None);
                },
            );
        });
    }

    {
        // 歌单级删除（subscribed = 取消收藏；owned = 远端删除成功后删本地行）
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui.as_weak();
        PlaylistMenu::get(ui).on_delete(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let menu = PlaylistMenu::get(&ui);
            let Ok(id) = menu.get_id().parse::<i64>() else {
                return;
            };
            let relation = menu.get_relation().to_string();
            let rt = Arc::clone(&runtime);
            player_bridge::dispatch_with_reply(
                &runtime,
                &ui_weak,
                Request::PlaylistWrite {
                    op: PlaylistWriteOp::Delete { id },
                },
                move |ui, result| {
                    apply_playlist_write(
                        ui,
                        &rt,
                        delete_outcome(&relation, &result),
                        Some((id, relation.as_str())),
                    );
                },
            );
        });
    }

    {
        // 新建歌单：命名弹层发来原样名称 → Create → Created(id) → 刷新 + 直达
        let runtime = Arc::clone(&runtime);
        let ui_weak = ui.as_weak();
        Data::get(ui).on_create_playlist(move |raw| {
            let Some(name) = playlist_name(&raw) else {
                // 全是空白：弹层的创建钮只按「非空」判可用，这里兜底给反馈
                if let Some(ui) = ui_weak.upgrade() {
                    player_bridge::show_feedback(&ui.as_weak(), "歌单名称不能为空".into());
                }
                return;
            };
            let rt = Arc::clone(&runtime);
            player_bridge::dispatch_with_reply(
                &runtime,
                &ui_weak,
                Request::PlaylistWrite {
                    op: PlaylistWriteOp::Create { name },
                },
                move |ui, result| apply_create_reply(ui, &rt, result),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_name_trims_and_rejects_blank() {
        assert_eq!(playlist_name("  夜路  "), Some("夜路".to_string()));
        assert_eq!(
            playlist_name(" driveway mixes "),
            Some("driveway mixes".to_string())
        );
        assert_eq!(playlist_name(""), None);
        assert_eq!(playlist_name("   \t\n"), None, "全是空白 = 空名");
    }

    #[test]
    fn rename_and_delete_outcomes_map_every_reply_shape() {
        assert_eq!(
            rename_outcome(&Ok(Response::Ok)),
            PlaylistWriteOutcome::Accepted("歌单已重命名".into())
        );
        let rejected = rename_outcome(&Ok(Response::Err {
            code: hmp_core::IpcErrorCode::Internal,
            message: "云端歌单不支持重命名".into(),
        }));
        assert_eq!(
            rejected,
            PlaylistWriteOutcome::Failed("重命名失败：云端歌单不支持重命名".into())
        );
        assert!(matches!(
            rename_outcome(&Err(BackendError::NoBackendBinary)),
            PlaylistWriteOutcome::Failed(_)
        ));

        assert_eq!(delete_success_text("subscribed"), "已取消收藏");
        assert_eq!(delete_success_text("owned"), "已提交删除，正在同步到云端");
        assert_eq!(delete_success_text("local"), "歌单已删除");
        assert_eq!(
            delete_outcome("local", &Ok(Response::Ok)),
            PlaylistWriteOutcome::Accepted("歌单已删除".into())
        );
        let failed = delete_outcome(
            "local",
            &Ok(Response::Err {
                code: hmp_core::IpcErrorCode::PlaylistNotFound,
                message: "歌单不存在".into(),
            }),
        );
        assert_eq!(
            failed,
            PlaylistWriteOutcome::Failed("删除歌单失败：歌单不存在".into())
        );
    }

    #[test]
    fn create_outcome_maps_every_reply_shape() {
        assert_eq!(
            create_outcome(&Ok(Response::Created(42))),
            CreateOutcome::Created(42)
        );
        let rejected = create_outcome(&Ok(Response::Err {
            code: hmp_core::IpcErrorCode::PlaylistNotFound,
            message: "boom".into(),
        }));
        assert_eq!(rejected, CreateOutcome::Failed("新建歌单失败：boom".into()));
        let unexpected = create_outcome(&Ok(Response::Ok));
        assert!(
            matches!(unexpected, CreateOutcome::Failed(_)),
            "非 Created 应答按未生效收敛"
        );
        let unreachable = create_outcome(&Err(BackendError::NoBackendBinary));
        assert!(matches!(unreachable, CreateOutcome::Failed(_)));
    }
}
