//! Unix socket 控制服务器（spec §4.2 `server.rs` / §5）。
//!
//! 长度前缀 JSON 帧；每连接独立任务；查询（Status/Queue）直接读
//! `EngineHandle.state_rx` 同步应答；Subscribe 后推送 `Event` 帧。

use std::path::PathBuf;

use hmp_core::ipc::{
    Event, IpcErrorCode, LoginQrState, MAX_FRAME, Request, Response, decode_frame, encode_frame,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::engine::EngineHandle;
use crate::transport::{IpcListener, IpcStream};

/// 端点：`$XDG_RUNTIME_DIR/hmp.sock`，回退 `/tmp/hmp-{uid}/hmp.sock`
/// （owner-only 目录，final review Finding 5）；Windows 为命名管道
/// `\\.\pipe\hmp`（实例不落盘，进程退出即消失）。与 serve.rs 一致，勿重复实现。
///
/// `HMP_IPC_ENDPOINT` 显式覆盖（非空即生效，两平台同语义）：serve 端绑定
/// 与 CLI/桌面客户端连接走同一函数，天然一致——用于测试隔离（真实 daemon
/// 占用默认端点时，沙箱 daemon 指到独立管道/socket）与便携化场景。
/// Windows 下任意路径会被 `transport::pipe_name` 确定性映射为
/// `\\.\pipe\hmp-<sanitized>`；Unix 下须是 socket 文件路径。
pub fn socket_path() -> PathBuf {
    if let Ok(ep) = std::env::var("HMP_IPC_ENDPOINT") {
        if !ep.is_empty() {
            return PathBuf::from(ep);
        }
    }
    // 平台块作尾表达式：Windows 只剩管道名分支，Unix 只剩 socket 路径分支。
    #[cfg(windows)]
    {
        PathBuf::from(r"\\.\pipe\hmp")
    }
    #[cfg(unix)]
    {
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
            if !dir.is_empty() {
                return PathBuf::from(dir).join("hmp.sock");
            }
        }
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/hmp-{uid}/hmp.sock"))
    }
}

/// 启动服务器（accept 循环；由 daemon 编排退出时机）。
pub async fn serve(mut listener: IpcListener, handle: EngineHandle) {
    loop {
        match listener.accept().await {
            Ok(stream) => {
                let handle = handle.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, handle).await {
                        tracing::debug!(%e, "connection handler ended");
                    }
                });
            }
            Err(e) => {
                tracing::error!(%e, "accept failed");
                break;
            }
        }
    }
}

/// 需要登录态的请求（服务器同步前置校验，spec §6）。
/// 本地源（`PlayRequest::Local`）与收藏/歌单写命令不要求 QQ 凭证
/// （本地先提交、离线意图合法）；reconcile 拉取 QQ 快照需要。
fn requires_credential(req: &Request) -> bool {
    match req {
        Request::Play(s) | Request::PlayNext(s) | Request::QueueAppend(s) => {
            // 本地源（Local/本地歌单）免凭证：离线意图合法；
            // 歌单内 QQ 曲目在曲目级 resolve_track 时再按凭证拦截。
            !s.is_local_source()
        }
        Request::PlayList { ids, .. } => {
            // 混排列表只要含 QQ 曲目即按 Play(Track) 同口径要求凭证
            //（QQ 取流必须登录；纯 local 列表离线可播）。
            ids.iter().any(|id| {
                hmp_core::TrackProvider::from_id(id.as_ref()) != hmp_core::TrackProvider::Local
            })
        }
        Request::LibrarySync => true,
        _ => false,
    }
}

/// 音质别名合法（`auto` 或 `AudioQuality` 别名；守护不认识的模式直接拒绝，
/// 避免 config.toml 落入损坏值——load 时会整体回默认）。
fn quality_alias_valid(mode: &str) -> bool {
    mode == "auto" || hmp_core::AudioQuality::from_alias(mode).is_some()
}

/// 单连接处理：请求/响应循环 + 订阅事件推送（reader 任务 + channel 并发版）。
///
/// 帧读取剥离到独立 reader 任务（阻塞 `read_frame`，逐帧经 channel 投递），
/// 主循环用 `select!` 同时监听下一帧与 `state_rx` 状态变更：订阅客户端空闲时
/// 仍能收到推送事件（不再有 100ms 轮询窗口停滞），请求也能即时应答
/// （无需等待轮询窗口兜底）。直接对 `read_frame` 做 `select!` 会取消进行中的
/// 读取并破坏帧边界，故采用 channel 中转。
async fn handle_connection(stream: IpcStream, mut handle: EngineHandle) -> std::io::Result<()> {
    let (mut rd, mut wr) = tokio::io::split(stream);
    let (frame_tx, mut frame_rx) = mpsc::channel::<std::io::Result<Option<Vec<u8>>>>(8);
    // reader 任务：阻塞读帧，逐帧投递；EOF/错误后退出（channel 关闭触发主循环收尾）。
    let reader = tokio::spawn(async move {
        loop {
            match read_frame(&mut rd).await {
                Ok(Some(f)) => {
                    if frame_tx.send(Ok(Some(f))).await.is_err() {
                        break; // 主循环已退出，停止投递
                    }
                }
                Ok(None) => {
                    let _ = frame_tx.send(Ok(None)).await;
                    break;
                }
                Err(e) => {
                    let _ = frame_tx.send(Err(e)).await;
                    break;
                }
            }
        }
    });
    let mut subscribed = false;
    let result: std::io::Result<()> = async {
        loop {
            tokio::select! {
                frame = frame_rx.recv() => {
                    let Some(frame) = frame else { break }; // 通道关闭（reader 已退出）
                    match frame {
                        Ok(Some(raw)) => {
                            handle_frame(&mut wr, &mut handle, raw, &mut subscribed).await?;
                        }
                        Ok(None) => break, // EOF
                        Err(e) => return Err(e),
                    }
                }
                // 订阅期间：状态变更与下一请求并发处理（推送不被请求读取阻塞）。
                _ = handle.state_rx.changed(), if subscribed => {
                    let ev = Event::StateChanged(handle.state_rx.borrow().clone());
                    write_frame(&mut wr, &ev).await?;
                }
                // 库变更代际推进 → 轻量事件（客户端按直读契约重查 sqlite）。
                _ = handle.library_rx.changed(), if subscribed => {
                    handle.library_rx.borrow_and_update();
                    write_frame(&mut wr, &Event::LibraryChanged).await?;
                }
            }
        }
        Ok(())
    }
    .await;
    reader.abort(); // 任何退出路径都终止 reader 任务（防泄漏）
    result
}

/// 处理一帧请求：查询直接应答；Subscribe 置位并推初始快照；Play 类做凭证前置
/// 校验后投递命令通道；解码失败回 BadRequest。订阅推送由主循环负责。
async fn handle_frame<W: AsyncWrite + Unpin>(
    wr: &mut W,
    handle: &mut EngineHandle,
    raw: Vec<u8>,
    subscribed: &mut bool,
) -> std::io::Result<()> {
    match decode_frame::<Request>(&raw) {
        Ok(Request::Status) => {
            // 先克隆出响应再 await，避免 watch::Ref 守卫跨 await（Send 约束）。
            let resp = Response::Status(handle.state_rx.borrow().clone());
            write_frame(wr, &resp).await?;
        }
        Ok(Request::Queue) => {
            let resp = Response::Queue(handle.queue_rx.borrow().clone());
            write_frame(wr, &resp).await?;
        }
        Ok(Request::QueueList { offset, limit }) => {
            // 纯 ID 分页（server 无媒体库引用；标题投影在 CLI 侧）。
            let snap = handle.queue_rx.borrow().clone();
            let total = snap.tracks.len();
            let items = snap
                .tracks
                .iter()
                .enumerate()
                .skip(offset)
                .take(limit)
                .map(|(i, t)| hmp_core::QueueEntry {
                    track_id: t.clone(),
                    is_current: snap.current == Some(i),
                })
                .collect();
            let resp = Response::QueueList(hmp_core::QueuePage {
                total,
                offset,
                items,
            });
            write_frame(wr, &resp).await?;
        }
        Ok(Request::Subscribe) => {
            *subscribed = true;
            // 先推初始快照，并标记为已见：防止引擎启动发布的 pending 版本让
            // `changed()` 立即再推一帧重复快照（两帧连读导致客户端解码失败）。
            let ev = Event::StateChanged(handle.state_rx.borrow_and_update().clone());
            write_frame(wr, &ev).await?;
        }
        // —— 媒体库写命令：本地先提交，QQ 由 SyncWorker 异步同步（spec §3.3）。
        // 与播放状态正交，server 直接操作媒体库（不经过引擎命令循环）。
        Ok(Request::Favorite {
            source,
            key,
            title,
            desired,
        }) => {
            let is_local = source == "local";
            let source_static: &'static str = if is_local { "local" } else { "qq" };
            let result = handle.library.as_ref().map(|lib| {
                let mut lib = lib.lock().unwrap();
                lib.upsert_track(&hmp_storage::TrackRow {
                    source: source_static,
                    source_key: key.clone(),
                    title,
                    album: None,
                    artist: None,
                    duration_ms: None,
                    cover_uri: None,
                    qq_song_id: None,
                    ..Default::default()
                })
                .and_then(|_| lib.set_relation("track", source_static, &key, "liked", desired))
                .and_then(|_| {
                    if is_local {
                        // 本地即事实：不进 outbox。
                        lib.mark_relation_synced("track", "local", &key, "liked")
                    } else {
                        Ok(())
                    }
                })
                .map_err(|e| e.to_string())
            });
            let resp = match result {
                Some(Ok(())) => {
                    // 收藏写落库 → 媒体库内容变更（客户端刷新"我喜欢"等页）。
                    handle.library_tx.send_modify(|g| *g += 1);
                    if !is_local {
                        if let Some(h) = &handle.sync_handle {
                            h.trigger();
                        }
                    }
                    Response::Ok
                }
                Some(Err(message)) => Response::Err {
                    code: IpcErrorCode::Internal,
                    message,
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "library unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::PlaylistWrite { op }) => {
            use hmp_core::PlaylistWriteOp;
            let mut trigger_sync = false;
            let result = handle
                .library
                .as_ref()
                .map(|lib| -> rusqlite::Result<Option<i64>> {
                    let mut lib = lib.lock().unwrap();
                    let r: rusqlite::Result<Option<i64>> = match op {
                        PlaylistWriteOp::Create { name } => lib.create_playlist(&name).map(Some),
                        PlaylistWriteOp::Rename { id, name } => match lib.playlist_relation(id) {
                            Ok(Some(r)) if r == "owned" || r == "subscribed" => {
                                Err(rusqlite::Error::InvalidQuery)
                            }
                            Ok(_) => lib.rename_playlist(id, &name).map(|_| None),
                            Err(e) => Err(e),
                        },
                        PlaylistWriteOp::Delete { id } => match lib.playlist_relation(id) {
                            Ok(Some(r)) if r == "owned" => {
                                // 行保留到远端删除成功（op 驱动）；pending + op 单事务。
                                lib.mark_pending_with_delete_op(id)?;
                                trigger_sync = true;
                                Ok(None)
                            }
                            Ok(Some(r)) if r == "subscribed" => {
                                // 取消收藏：删本地行 + relations outbox 单事务
                                // （unfav 同步成功前 reconcile 不覆盖 pending，不会复活）。
                                let remote = lib.playlist_remote_id(id)?;
                                lib.unfavorite_playlist(id, remote.as_deref())?;
                                if remote.is_some() {
                                    trigger_sync = true;
                                }
                                Ok(None)
                            }
                            Ok(_) => lib.delete_playlist(id).map(|_| None),
                            Err(e) => Err(e),
                        },
                        PlaylistWriteOp::AddTrack {
                            id,
                            source,
                            key,
                            title,
                        } => {
                            let rel = lib.playlist_relation(id)?.unwrap_or_default();
                            if rel == "subscribed" {
                                return Err(rusqlite::Error::InvalidQuery);
                            }
                            // QQ owned 歌单只接受 QQ 曲目：local 曲目无 QQ song id，
                            // 写入 outbox 会永久 error/重试（本地行也会成幽灵）。
                            if rel == "owned" && source == "local" {
                                return Err(rusqlite::Error::InvalidQuery);
                            }
                            let source_static: &'static str = match source.as_str() {
                                "local" => "local",
                                _ => "qq",
                            };
                            if rel == "owned" {
                                // 单事务：本地关联 + outbox 入队原子（无窗口）。
                                let song_id = lib.qq_song_id("qq", &key)?;
                                lib.add_owned_track_with_op(
                                    id,
                                    source_static,
                                    &key,
                                    &title,
                                    song_id,
                                )?;
                                trigger_sync = true;
                            } else {
                                lib.add_playlist_track(id, source_static, &key, &title)?;
                            }
                            Ok(None)
                        }
                        PlaylistWriteOp::RemoveTrack { id, position } => {
                            let rel = lib.playlist_relation(id)?.unwrap_or_default();
                            if rel == "subscribed" {
                                return Err(rusqlite::Error::InvalidQuery);
                            }
                            let song_key = lib.track_key_at(id, position)?;
                            if rel == "owned" {
                                if let Some(key) = song_key {
                                    // local 曲目在远端无对应物：只删本地行，不入 outbox
                                    // （否则 song_id 恒 None → 永久 error 重试）。
                                    if !key.starts_with("local:") {
                                        let song_id = lib.qq_song_id("qq", &key)?;
                                        lib.remove_owned_track_with_op(
                                            id, position, &key, song_id,
                                        )?;
                                        trigger_sync = true;
                                    } else {
                                        lib.remove_playlist_track(id, position)?;
                                    }
                                } else {
                                    lib.remove_playlist_track(id, position)?;
                                }
                            } else {
                                lib.remove_playlist_track(id, position)?;
                            }
                            Ok(None)
                        }
                    };
                    r
                });
            let resp = match result {
                Some(Ok(Some(id))) => {
                    // 歌单结构落库 → 媒体库内容变更。
                    handle.library_tx.send_modify(|g| *g += 1);
                    if trigger_sync {
                        if let Some(h) = &handle.sync_handle {
                            h.trigger();
                        }
                    }
                    Response::Created(id)
                }
                Some(Ok(None)) => {
                    handle.library_tx.send_modify(|g| *g += 1);
                    if trigger_sync {
                        if let Some(h) = &handle.sync_handle {
                            h.trigger();
                        }
                    }
                    Response::Ok
                }
                Some(Err(rusqlite::Error::InvalidQuery)) => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "remote QQ playlists are read-only (subscribed: unlike only; owned: no rename)".into(),
                },
                Some(Err(e)) => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: e.to_string(),
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "library unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::LibrarySync) => {
            // 前置校验：reconcile 需要登录态（requires_credential 只作用于通用分支）。
            if !(handle.credential_ok)() {
                write_frame(
                    wr,
                    &Response::Err {
                        code: IpcErrorCode::NotLoggedIn,
                        message: "not logged in; run `hmp login` first".into(),
                    },
                )
                .await?;
                return Ok(());
            }
            let resp = match &handle.sync_handle {
                Some(h) => {
                    h.reconcile();
                    Response::Ok
                }
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "sync worker unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        // —— 评论（spec §6）：读走 TTL cache；写直发 QQ。
        Ok(Request::CommentList {
            mid,
            sort,
            page,
            num,
        }) => {
            let resp = match &handle.comment {
                Some(svc) => match svc.list(&mid, &sort, page, num).await {
                    Ok(page) => Response::CommentList(page),
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "comment service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::CommentPost {
            mid,
            content,
            reply_cmt_id,
        }) => {
            let resp = match &handle.comment {
                Some(svc) => match svc.post(&mid, &content, reply_cmt_id.as_deref()).await {
                    Ok(_) => Response::Ok,
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "comment service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::CommentDelete { cm_id }) => {
            let resp = match &handle.comment {
                Some(svc) => match svc.delete(&cm_id).await {
                    Ok(()) => Response::Ok,
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "comment service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        // —— 内容读服务（AUDIT §8.2/8.3/8.6/8.4：daemon 持凭证统一出网）———
        Ok(Request::Search { keyword }) => {
            let resp = match &handle.content {
                Some(svc) => match svc.search(&keyword).await {
                    Ok(page) => Response::Search(page),
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "content service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::LyricGet { id, title, artist }) => {
            let resp = match &handle.content {
                Some(svc) => match svc.track_lyric(&id, &title, &artist).await {
                    Ok(page) => Response::Lyric(page),
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "content service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::AccountStatus) => {
            let resp = match &handle.content {
                Some(svc) => match svc.account_status().await {
                    Ok(info) => Response::AccountStatus(info),
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "content service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        // —— 扫码登录 / 登出（凭证操作统一在 daemon；AUDIT §8.6 延伸）———
        Ok(Request::LoginQrStart) => {
            let resp = match &handle.login {
                Some(svc) => match svc.start().await {
                    Ok(session) => Response::LoginQr(session),
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "login service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::LoginQrPoll) => {
            let resp = match &handle.login {
                Some(svc) => {
                    let state = svc.poll().await;
                    if state.status == LoginQrState::STATUS_DONE {
                        // 登录成功：账号缓存失效（下次 AccountStatus 重新出网）
                        // + QQ 用户库 reconcile（spec §4：有凭证即拉快照）。
                        if let Some(content) = &handle.content {
                            content.invalidate_account_cache();
                        }
                        if let Some(sync) = &handle.sync_handle {
                            sync.reconcile();
                        }
                    }
                    Response::LoginQrState(state)
                }
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "login service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::LoginQrCancel) => {
            let resp = match &handle.login {
                Some(svc) => {
                    svc.cancel();
                    Response::Ok
                }
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "login service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::Logout) => {
            let resp = match &handle.login {
                Some(svc) => match svc.logout().await {
                    Ok(()) => {
                        if let Some(content) = &handle.content {
                            content.invalidate_account_cache();
                        }
                        Response::Ok
                    }
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "login service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::CoverGet { url }) => {
            let resp = match &handle.content {
                Some(svc) => match svc.cover(&url).await {
                    Ok(path) => Response::Cover(path),
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "content service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        // —— 发现页/榜单/猜你喜欢（daemon 统一出网，AUDIT §8.2 同源）———
        Ok(Request::DiscoverGet {
            songlist_page,
            new_song_type,
        }) => {
            let resp = match &handle.content {
                Some(svc) => match svc.discover(songlist_page, new_song_type).await {
                    Ok(page) => Response::Discover(page),
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "content service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::TopCategoryGet) => {
            let resp = match &handle.content {
                Some(svc) => match svc.top_category().await {
                    Ok(page) => Response::TopCategory(page),
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "content service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::TopDetailGet { top_id, num, page }) => {
            let resp = match &handle.content {
                Some(svc) => match svc.top_detail(top_id, num, page).await {
                    Ok(page) => Response::TopDetail(page),
                    Err(message) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message,
                    },
                },
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "content service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(Request::GuessGet { page }) => {
            let resp = match &handle.content {
                Some(svc) => {
                    if !(handle.credential_ok)() {
                        Response::Err {
                            code: IpcErrorCode::NotLoggedIn,
                            message: "猜你喜欢需登录".into(),
                        }
                    } else {
                        match svc.guess(page).await {
                            Ok(page) => Response::Guess(page),
                            Err(message) => Response::Err {
                                code: IpcErrorCode::Internal,
                                message,
                            },
                        }
                    }
                }
                None => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "content service unavailable".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        // —— 音质偏好（AUDIT §8.7：daemon 落 config.toml，UI 只发意图）———
        Ok(Request::QualityGet) => {
            let q = hmp_storage::Config::load().quality;
            let resp = Response::Quality(hmp_core::QualityPrefDto {
                mode: q.mode,
                fallback: q.fallback,
            });
            write_frame(wr, &resp).await?;
        }
        Ok(Request::QualitySet { mode, fallback }) => {
            let resp = match quality_alias_valid(&mode) {
                true => {
                    let mut config = hmp_storage::Config::load();
                    config.quality = hmp_storage::QualityPref { mode, fallback };
                    match config.save() {
                        Ok(()) => {
                            // daemon 每曲解析时重读 config，此处无需失效通知。
                            Response::Ok
                        }
                        Err(e) => Response::Err {
                            code: IpcErrorCode::Internal,
                            message: e.to_string(),
                        },
                    }
                }
                false => Response::Err {
                    code: IpcErrorCode::BadRequest,
                    message: format!("unknown quality mode: {mode}"),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Ok(req) => {
            if requires_credential(&req) && !(handle.credential_ok)() {
                write_frame(
                    wr,
                    &Response::Err {
                        code: IpcErrorCode::NotLoggedIn,
                        message: "not logged in; run `hmp login` first".into(),
                    },
                )
                .await?;
                return Ok(());
            }
            let resp = match handle.command_tx.send(req) {
                Ok(_) => Response::Ok,
                Err(_) => Response::Err {
                    code: IpcErrorCode::Internal,
                    message: "engine has exited".into(),
                },
            };
            write_frame(wr, &resp).await?;
        }
        Err(e) => {
            write_frame(
                wr,
                &Response::Err {
                    code: IpcErrorCode::BadRequest,
                    message: e.to_string(),
                },
            )
            .await?;
        }
    }
    Ok(())
}

/// 读一帧（含 4 字节长度前缀）；EOF 返回 `None`。
async fn read_frame<R: AsyncRead + Unpin>(stream: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME - 4 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;
    let mut frame = Vec::with_capacity(4 + len);
    frame.extend_from_slice(&len_buf);
    frame.extend_from_slice(&payload);
    Ok(Some(frame))
}

/// 写一帧。
async fn write_frame<W: AsyncWrite + Unpin>(
    stream: &mut W,
    msg: &impl serde::Serialize,
) -> std::io::Result<()> {
    let frame = encode_frame(msg).map_err(|e| std::io::Error::other(e.to_string()))?;
    stream.write_all(&frame).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::PlaybackEngine;
    use crate::player::{EngineError, PlaybackDriver, ResolvedTrack, SourceResolver};
    use hmp_core::ipc::{Event, Request, Response};
    use hmp_core::{
        IpcErrorCode, PlayRequest, PlaybackState, PlaybackStatus, PlayerCommand, Track, TrackId,
    };
    use hmp_core::{LoadRequest, PlayerEvent};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use tokio::sync::{broadcast, watch};

    /// 最小 fake 播放驱动（本模块测试专用）。
    struct SDriver {
        state_tx: watch::Sender<PlaybackState>,
        events_tx: broadcast::Sender<PlayerEvent>,
    }
    impl PlaybackDriver for SDriver {
        fn load(&self, r: LoadRequest) {
            // 同步应用（模拟真实驱动装载臂）：current + Playing。
            let (track, quality) = (r.track, r.quality);
            self.state_tx.send_modify(|s| {
                s.status = PlaybackStatus::Playing;
                s.current = Some(track);
                s.actual_quality = Some(quality);
            });
        }
        fn play(&self) {}
        fn pause(&self) {}
        fn seek(&self, _p: std::time::Duration) {}
        fn stop(&self) {}
        fn set_volume(&self, _v: f64) {}
        fn command(&self, _c: PlayerCommand) {}
        fn shutdown(&self) {}
        fn subscribe_state(&self) -> watch::Receiver<PlaybackState> {
            self.state_tx.subscribe()
        }
        fn subscribe_events(&self) -> broadcast::Receiver<PlayerEvent> {
            self.events_tx.subscribe()
        }
    }

    /// 最小 fake 解析器（不触网）。
    #[derive(Debug)]
    struct SResolver;

    impl SourceResolver for SResolver {
        fn resolve_source_ids(
            &self,
            _s: &PlayRequest,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<hmp_core::TrackStub>, EngineError>> + Send + '_>>
        {
            Box::pin(async {
                Ok(vec![hmp_core::TrackStub {
                    id: TrackId::new("a"),
                    title: "a".into(),
                    artists: Vec::new(),
                    album: None,
                    duration_ms: None,
                }])
            })
        }
        fn resolve_track(
            &self,
            id: &TrackId,
        ) -> Pin<Box<dyn Future<Output = Result<ResolvedTrack, EngineError>> + Send + '_>> {
            // 克隆 id：让 future 持有数据，不借用参数。
            let id = id.clone();
            Box::pin(async move {
                Ok(ResolvedTrack {
                    track: Track {
                        id: id.clone(),
                        title: format!("t-{id}"),
                        artists: vec![],
                        album: None,
                        duration: Some(std::time::Duration::from_secs(60)),
                        cover: None,
                        url: Some(format!("fake://{id}")),
                        available_qualities: vec![],
                    },
                    uri: format!("fake://{id}"),
                    media: None,
                    quality: hmp_core::AudioQuality::Mp3_128,
                    replaygain_db: None,
                })
            })
        }
    }

    async fn test_engine(cred_ok: bool) -> EngineHandle {
        let (state_tx, _) = watch::channel(PlaybackState::default());
        let (events_tx, _) = broadcast::channel(16);
        let driver = Arc::new(SDriver {
            state_tx,
            events_tx,
        });
        PlaybackEngine::start(driver, Arc::new(SResolver), Arc::new(move || cred_ok))
    }

    async fn temp_socket() -> (PathBuf, IpcListener) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hmp-test.sock");
        let listener = IpcListener::bind(&path).unwrap();
        // TempDir 必须保持存活到测试结束：drop 会删除 socket 文件，
        // 使后续 connect 失败（ENOENT）。Windows 管道名映射在传输层内做。
        std::mem::forget(dir);
        (path, listener)
    }

    /// 连接 → 发送一帧 → 读一帧响应（每次新建连接）。
    async fn request(sock: &std::path::Path, req: &Request) -> Response {
        let mut stream = IpcStream::connect(sock).await.unwrap();
        stream.write_all(&encode_frame(req).unwrap()).await.unwrap();
        let mut buf = vec![0u8; 65536];
        let n = stream.read(&mut buf).await.unwrap();
        decode_frame::<Response>(&buf[..n]).unwrap()
    }

    #[tokio::test]
    async fn status_returns_daemon_state() {
        let (sock, listener) = temp_socket().await;
        let handle = test_engine(true).await;
        tokio::spawn(async move { serve(listener, handle).await });
        let resp = request(&sock, &Request::Status).await;
        assert!(matches!(resp, Response::Status(_)));
    }

    #[tokio::test]
    async fn queue_query_returns_snapshot() {
        let (sock, listener) = temp_socket().await;
        let handle = test_engine(true).await;
        tokio::spawn(async move { serve(listener, handle).await });
        let resp = request(&sock, &Request::Queue).await;
        assert!(matches!(resp, Response::Queue(_)));
    }

    #[tokio::test]
    async fn queue_list_pages_with_current_marker() {
        let (sock, listener) = temp_socket().await;
        let handle = test_engine(true).await;
        handle
            .cmd(Request::Play(PlayRequest::Track(TrackId::new("a"))))
            .await
            .unwrap();
        // 队列 [a]；等引擎发布后查询分页。
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        tokio::spawn(async move { serve(listener, handle).await });

        let resp = request(
            &sock,
            &Request::QueueList {
                offset: 0,
                limit: 10,
            },
        )
        .await;
        let Response::QueueList(page) = resp else {
            panic!("期望 QueueList 响应");
        };
        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].track_id.as_ref(), "a");
        assert!(page.items[0].is_current, "当前曲应标记");

        // 越界页：空 items、total 不变。
        let resp = request(
            &sock,
            &Request::QueueList {
                offset: 5,
                limit: 10,
            },
        )
        .await;
        let Response::QueueList(page) = resp else {
            panic!("期望 QueueList 响应");
        };
        assert_eq!(page.total, 1);
        assert!(page.items.is_empty());
    }

    #[tokio::test]
    async fn library_sync_requires_login() {
        let (sock, listener) = temp_socket().await;
        let handle = test_engine(false).await; // 无凭证
        tokio::spawn(async move { serve(listener, handle).await });
        let resp = request(&sock, &Request::LibrarySync).await;
        assert!(
            matches!(
                resp,
                Response::Err {
                    code: IpcErrorCode::NotLoggedIn,
                    ..
                }
            ),
            "未登录时 library sync 应被前置校验拒绝: {resp:?}"
        );
    }

    #[tokio::test]
    async fn subscribe_pushes_initial_and_changes() {
        let (sock, listener) = temp_socket().await;
        let (state_tx, _) = watch::channel(PlaybackState::default());
        let (events_tx, _) = broadcast::channel(16);
        let driver = Arc::new(SDriver {
            state_tx: state_tx.clone(),
            events_tx,
        });
        let handle = PlaybackEngine::start(driver.clone(), Arc::new(SResolver), Arc::new(|| true));
        tokio::spawn(async move { serve(listener, handle).await });
        let mut stream = IpcStream::connect(&sock).await.unwrap();
        stream
            .write_all(&encode_frame(&Request::Subscribe).unwrap())
            .await
            .unwrap();
        let mut buf = vec![0u8; 65536];
        let n = stream.read(&mut buf).await.unwrap();
        let ev: Event = decode_frame(&buf[..n]).unwrap();
        assert!(matches!(ev, Event::StateChanged(_)));
        // 触发状态变更 → 订阅帧（select 轮询间隔 100ms，等 300ms）
        state_tx.send_modify(|s| s.status = PlaybackStatus::Paused);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let n = stream.read(&mut buf).await.unwrap();
        let ev2: Event = decode_frame(&buf[..n]).unwrap();
        assert!(matches!(ev2, Event::StateChanged(_)));
    }

    /// 回归（review Finding 1）：订阅后不发任何请求，状态变更仍须推送。
    /// 旧实现（read_frame 先行 + 100ms select 窗口）在窗口关闭后停滞，
    /// 该测试在旧代码上会超时失败；reader 任务版在空闲时也能推送。
    #[tokio::test]
    async fn subscribe_receives_change_without_further_requests() {
        let (sock, listener) = temp_socket().await;
        let (state_tx, _) = watch::channel(PlaybackState::default());
        let (events_tx, _) = broadcast::channel(16);
        let driver = Arc::new(SDriver {
            state_tx: state_tx.clone(),
            events_tx,
        });
        let handle = PlaybackEngine::start(driver.clone(), Arc::new(SResolver), Arc::new(|| true));
        tokio::spawn(async move { serve(listener, handle).await });
        let mut stream = IpcStream::connect(&sock).await.unwrap();
        stream
            .write_all(&encode_frame(&Request::Subscribe).unwrap())
            .await
            .unwrap();
        let mut buf = vec![0u8; 65536];
        let n = stream.read(&mut buf).await.unwrap();
        let ev: Event = decode_frame(&buf[..n]).unwrap();
        assert!(matches!(ev, Event::StateChanged(_)));
        // 等待超过旧实现的 100ms 轮询窗口，确保读端已无待处理请求。
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        // 触发状态变更；期间不发送任何请求，仍须收到推送帧。
        state_tx.send_modify(|s| s.status = PlaybackStatus::Paused);
        let read =
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buf)).await;
        let n = read
            .expect("订阅后状态变更未推送（停滞）")
            .expect("读推送帧失败");
        let ev2: Event = decode_frame(&buf[..n]).unwrap();
        assert!(matches!(ev2, Event::StateChanged(_)));
    }

    /// 回归（review Finding 1）：订阅状态下请求不受轮询窗口拖累，即时应答。
    /// 旧实现每次请求需等满 100ms sleep 兜底；reader 任务版直接应答。
    #[tokio::test]
    async fn subscribed_request_answered_without_poll_delay() {
        let (sock, listener) = temp_socket().await;
        let (state_tx, _) = watch::channel(PlaybackState::default());
        let (events_tx, _) = broadcast::channel(16);
        let driver = Arc::new(SDriver {
            state_tx: state_tx.clone(),
            events_tx,
        });
        let handle = PlaybackEngine::start(driver.clone(), Arc::new(SResolver), Arc::new(|| true));
        tokio::spawn(async move { serve(listener, handle).await });
        let mut stream = IpcStream::connect(&sock).await.unwrap();
        stream
            .write_all(&encode_frame(&Request::Subscribe).unwrap())
            .await
            .unwrap();
        let mut buf = vec![0u8; 65536];
        let n = stream.read(&mut buf).await.unwrap();
        let _ev: Event = decode_frame(&buf[..n]).unwrap();
        // 订阅后发送 Status，应答须在 100ms 内（无状态变更、无轮询等待）。
        stream
            .write_all(&encode_frame(&Request::Status).unwrap())
            .await
            .unwrap();
        let read =
            tokio::time::timeout(std::time::Duration::from_millis(100), stream.read(&mut buf))
                .await;
        let n = read.expect("订阅后请求应答超过 100ms").expect("读响应失败");
        let resp: Response = decode_frame(&buf[..n]).unwrap();
        assert!(matches!(resp, Response::Status(_)));
    }

    #[tokio::test]
    async fn malformed_frame_gets_bad_request() {
        let (sock, listener) = temp_socket().await;
        let handle = test_engine(true).await;
        tokio::spawn(async move { serve(listener, handle).await });
        let mut stream = IpcStream::connect(&sock).await.unwrap();
        // 长度 4 + 非法 JSON（非 Request）→ decode 失败 → BadRequest
        stream
            .write_all(&[4, 0, 0, 0, b'j', b'u', b'n', b'k'])
            .await
            .unwrap();
        let mut buf = vec![0u8; 65536];
        let n = stream.read(&mut buf).await.unwrap();
        let resp: Response = decode_frame(&buf[..n]).unwrap();
        assert!(matches!(
            resp,
            Response::Err {
                code: IpcErrorCode::BadRequest,
                ..
            }
        ));
    }

    /// 内容读服务未注入（测试引擎）→ 诚实报"不可用"，不静默成功。
    #[tokio::test]
    async fn content_reads_report_unavailable_without_service() {
        let (sock, listener) = temp_socket().await;
        let handle = test_engine(true).await;
        tokio::spawn(async move { serve(listener, handle).await });
        for req in [
            Request::Search {
                keyword: "x".into(),
            },
            Request::LyricGet {
                id: "m".into(),
                title: "x".into(),
                artist: String::new(),
            },
            Request::AccountStatus,
            Request::CoverGet {
                url: "https://y.gtimg.cn/a.jpg".into(),
            },
        ] {
            let resp = request(&sock, &req).await;
            assert!(
                matches!(resp, Response::Err { ref message, .. } if message.contains("unavailable")),
                "未注入服务应报不可用: {req:?} → {resp:?}"
            );
        }
    }

    /// 收藏写落库 → 订阅端收到 `LibraryChanged`（AUDIT §8.9）。
    #[tokio::test]
    async fn favorite_write_pushes_library_changed() {
        let (sock, listener) = temp_socket().await;
        let (state_tx, _) = watch::channel(PlaybackState::default());
        let (events_tx, _) = broadcast::channel(16);
        let driver = Arc::new(SDriver {
            state_tx,
            events_tx,
        });
        let library = Arc::new(Mutex::new(
            hmp_storage::LibraryDb::open_in_memory().unwrap(),
        ));
        let handle = PlaybackEngine::start_with_library(
            driver,
            Arc::new(SResolver),
            Arc::new(|| true),
            Some(library.clone()),
            None,
        );
        let mut handle = handle;
        handle.library = Some(library);
        tokio::spawn(async move { serve(listener, handle).await });

        // 连接 A 订阅；连接 B 写收藏 → A 收到 LibraryChanged。
        let mut sub = IpcStream::connect(&sock).await.unwrap();
        sub.write_all(&encode_frame(&Request::Subscribe).unwrap())
            .await
            .unwrap();
        let mut buf = vec![0u8; 65536];
        let n = sub.read(&mut buf).await.unwrap();
        assert!(matches!(
            decode_frame::<Event>(&buf[..n]).unwrap(),
            Event::StateChanged(_)
        ));
        let resp = request(
            &sock,
            &Request::Favorite {
                source: "local".into(),
                key: "local:/tmp/a.flac".into(),
                title: "a".into(),
                desired: true,
            },
        )
        .await;
        assert!(matches!(resp, Response::Ok));

        let mut saw_library_changed = false;
        for _ in 0..4 {
            let read = tokio::time::timeout(std::time::Duration::from_secs(2), sub.read(&mut buf))
                .await
                .expect("库变更事件未推送（超时）");
            let n = read.expect("读事件失败");
            if let Event::LibraryChanged = decode_frame::<Event>(&buf[..n]).unwrap() {
                saw_library_changed = true;
                break;
            }
        }
        assert!(saw_library_changed, "收藏写后应推 LibraryChanged");
    }

    /// 音质别名守卫：auto/合法别名放行，未知模式拒绝（防 config 落坏值）。
    #[test]
    fn quality_alias_guard() {
        assert!(quality_alias_valid("auto"));
        for alias in ["master", "hires", "atmos", "flac", "aac", "320", "128"] {
            assert!(quality_alias_valid(alias), "合法别名: {alias}");
        }
        assert!(!quality_alias_valid("ultra"));
        assert!(!quality_alias_valid(""));
    }

    #[tokio::test]
    async fn play_without_credentials_returns_not_logged_in() {
        let (sock, listener) = temp_socket().await;
        let handle = test_engine(false).await;
        tokio::spawn(async move { serve(listener, handle).await });
        let resp = request(
            &sock,
            &Request::Play(PlayRequest::Track(TrackId::new("m1"))),
        )
        .await;
        assert!(matches!(
            resp,
            Response::Err {
                code: IpcErrorCode::NotLoggedIn,
                ..
            }
        ));
    }

    /// 未登录时：QQ 源被前置拒绝，本地源放行（登录门按 provider，C2）。
    #[tokio::test]
    async fn local_play_without_credentials_is_allowed() {
        let (sock, listener) = temp_socket().await;
        let handle = test_engine(false).await;
        tokio::spawn(async move { serve(listener, handle).await });
        // 本地源不要求 QQ 登录。
        let resp = request(
            &sock,
            &Request::Play(PlayRequest::Local(TrackId::new("local:/tmp/x.mp3"))),
        )
        .await;
        assert!(
            matches!(resp, Response::Ok),
            "本地播放不应要求登录，实际: {resp:?}"
        );
        // 同一连接 QQ 源仍被拒。
        let resp = request(
            &sock,
            &Request::Play(PlayRequest::Track(TrackId::new("m1"))),
        )
        .await;
        assert!(matches!(
            resp,
            Response::Err {
                code: IpcErrorCode::NotLoggedIn,
                ..
            }
        ));
    }

    /// QQ owned 歌单只接受 QQ 曲目：AddTrack local 被拒；
    /// RemoveTrack local 曲目只删本地行、不入 outbox。
    #[tokio::test]
    async fn owned_playlist_rejects_local_tracks() {
        let (sock, listener) = temp_socket().await;
        let (state_tx, _) = watch::channel(PlaybackState::default());
        let (events_tx, _) = broadcast::channel(16);
        let driver = Arc::new(SDriver {
            state_tx,
            events_tx,
        });
        let library = Arc::new(Mutex::new(
            hmp_storage::LibraryDb::open_in_memory().unwrap(),
        ));
        let handle = PlaybackEngine::start_with_library(
            driver,
            Arc::new(SResolver),
            Arc::new(|| true),
            Some(library.clone()),
            None,
        );
        let mut handle = handle;
        handle.library = Some(library.clone()); // 与 daemon.rs 启动后接线一致
        tokio::spawn(async move { serve(listener, handle).await });

        // 构造 owned 歌单（reconcile 路径：remote_id + relation=owned）。
        let owned_id = {
            let mut lib = library.lock().unwrap();
            lib.reconcile_playlist("dir-1", "我的歌单", "owned")
                .unwrap()
        };

        // AddTrack local → 拒绝（InvalidQuery 映射为 Err）。
        let resp = request(
            &sock,
            &Request::PlaylistWrite {
                op: hmp_core::PlaylistWriteOp::AddTrack {
                    id: owned_id,
                    source: "local".into(),
                    key: "local:/tmp/a.flac".into(),
                    title: "a".into(),
                },
            },
        )
        .await;
        assert!(
            matches!(resp, Response::Err { .. }),
            "local 曲目进 owned 歌单应被拒: {resp:?}"
        );

        // 直接落一条 local 曲目（模拟历史数据），RemoveTrack 只删本地行。
        {
            let mut lib = library.lock().unwrap();
            lib.add_playlist_track(owned_id, "local", "local:/tmp/a.flac", "a")
                .unwrap();
        }
        let resp = request(
            &sock,
            &Request::PlaylistWrite {
                op: hmp_core::PlaylistWriteOp::RemoveTrack {
                    id: owned_id,
                    position: 0,
                },
            },
        )
        .await;
        assert!(
            matches!(resp, Response::Ok),
            "本地曲目移除应成功（不入 outbox）: {resp:?}"
        );
        let mut lib = library.lock().unwrap();
        assert_eq!(
            lib.playlist_ops_pending().unwrap().len(),
            0,
            "local 曲目不得产生远端 op"
        );
        assert_eq!(
            lib.playlist_tracks(owned_id).unwrap().len(),
            0,
            "本地行应被删除"
        );
    }

    /// `HMP_IPC_ENDPOINT` 显式覆盖（非空生效/空值忽略；回归守护：serve 与
    /// CLI/桌面共用本函数，覆盖即两端一致，测试隔离依赖此行为）。
    #[test]
    fn socket_path_honors_hmp_ipc_endpoint_override() {
        // SAFETY: 单线程测试进程内串行执行；用唯一值避免与其他测试互相干扰。
        unsafe {
            std::env::set_var("HMP_IPC_ENDPOINT", r"\\.\pipe\hmp-test-override");
        }
        assert_eq!(
            socket_path(),
            PathBuf::from(r"\\.\pipe\hmp-test-override"),
            "非空覆盖必须生效"
        );
        unsafe {
            std::env::set_var("HMP_IPC_ENDPOINT", "");
        }
        let fallback = socket_path();
        #[cfg(windows)]
        assert_eq!(fallback, PathBuf::from(r"\\.\pipe\hmp"), "空值回退默认管道");
        #[cfg(unix)]
        assert!(
            fallback.ends_with("hmp.sock"),
            "空值回退平台默认端点: {fallback:?}"
        );
        unsafe {
            std::env::remove_var("HMP_IPC_ENDPOINT");
        }
    }
}

#[cfg(test)]
mod credential_policy_tests {
    use super::requires_credential;
    use hmp_core::{AlbumId, PlayRequest, Request, TrackId};

    #[test]
    fn every_local_play_source_bypasses_qq_credentials() {
        let sources = [
            PlayRequest::Local(TrackId::new("local:C:\\Music\\song.flac")),
            PlayRequest::Track(TrackId::new("local:C:\\Music\\song.flac")),
            PlayRequest::Album(AlbumId::new("local:本地专辑")),
            PlayRequest::LibraryPlaylist(1),
        ];

        for source in sources {
            for request in [
                Request::Play(source.clone()),
                Request::PlayNext(source.clone()),
                Request::QueueAppend(source.clone()),
            ] {
                assert!(
                    !requires_credential(&request),
                    "本地播放源不应要求 QQ 登录: {request:?}"
                );
            }
        }
    }
}
