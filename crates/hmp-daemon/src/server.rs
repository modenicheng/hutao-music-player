//! Unix socket 控制服务器（spec §4.2 `server.rs` / §5）。
//!
//! 长度前缀 JSON 帧；每连接独立任务；查询（Status/Queue）直接读
//! `EngineHandle.state_rx` 同步应答；Subscribe 后推送 `Event` 帧。

use std::path::PathBuf;

use hmp_core::ipc::{
    Event, IpcErrorCode, MAX_FRAME, Request, Response, decode_frame, encode_frame,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use crate::engine::EngineHandle;

/// socket 路径：`$XDG_RUNTIME_DIR/hmp.sock`，回退 `/tmp/hmp-{uid}.sock`。
pub fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("hmp.sock");
        }
    }
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/hmp-{uid}.sock"))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from("/tmp/hmp.sock")
    }
}

/// 启动服务器（accept 循环；由 daemon 编排退出时机）。
pub async fn serve(listener: UnixListener, handle: EngineHandle) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let handle = handle.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, handle).await {
                        tracing::debug!(%e, "连接处理结束");
                    }
                });
            }
            Err(e) => {
                tracing::error!(%e, "accept 失败");
                break;
            }
        }
    }
}

/// 需要登录态的请求（服务器同步前置校验，spec §6）。
fn is_play_request(req: &Request) -> bool {
    matches!(
        req,
        Request::Play(_) | Request::PlayNext(_) | Request::QueueAppend(_)
    )
}

/// 单连接处理：请求/响应循环 + 可选订阅推送（select 版，Task 3 Step 3）。
///
/// 订阅后每处理一帧请求就并发等待状态变更（100ms 轮询兜底 watch 边界），
/// 事件帧经同一 socket 推送；客户端随后续请求或断开驱动循环前进。
async fn handle_connection(
    mut stream: UnixStream,
    mut handle: EngineHandle,
) -> std::io::Result<()> {
    let mut subscribed = false;
    loop {
        // 读一帧（异步；EOF 结束连接）
        let frame = read_frame(&mut stream).await?;
        let Some(frame) = frame else { return Ok(()) };

        let next_req = decode_frame::<Request>(&frame);
        match next_req {
            Ok(Request::Status) => {
                // 先克隆出响应再 await，避免 watch::Ref 守卫跨 await（Send 约束）。
                let resp = Response::Status(handle.state_rx.borrow().clone());
                write_frame(&mut stream, &resp).await?;
            }
            Ok(Request::Queue) => {
                let resp = Response::Queue(handle.state_rx.borrow().queue.clone());
                write_frame(&mut stream, &resp).await?;
            }
            Ok(Request::Subscribe) => {
                subscribed = true;
                // 先推初始快照，并标记为已见：防止引擎启动发布的 pending 版本让
                // `changed()` 立即再推一帧重复快照（两帧连读导致客户端解码失败）。
                let ev = Event::StateChanged(handle.state_rx.borrow_and_update().clone());
                write_frame(&mut stream, &ev).await?;
            }
            Ok(req) => {
                if is_play_request(&req) && !(handle.credential_ok)() {
                    write_frame(
                        &mut stream,
                        &Response::Err {
                            code: IpcErrorCode::NotLoggedIn,
                            message: "未登录，请先运行 hmp login".into(),
                        },
                    )
                    .await?;
                    continue;
                }
                let resp = match handle.command_tx.send(req) {
                    Ok(_) => Response::Ok,
                    Err(_) => Response::Err {
                        code: IpcErrorCode::Internal,
                        message: "引擎已退出".into(),
                    },
                };
                write_frame(&mut stream, &resp).await?;
            }
            Err(e) => {
                write_frame(
                    &mut stream,
                    &Response::Err {
                        code: IpcErrorCode::BadRequest,
                        message: e.to_string(),
                    },
                )
                .await?;
            }
        }

        // 订阅期间：状态变更与下一请求并发等待（100ms 轮询兜底）。
        if subscribed {
            tokio::select! {
                _ = handle.state_rx.changed() => {
                    let ev = Event::StateChanged(handle.state_rx.borrow().clone());
                    write_frame(&mut stream, &ev).await?;
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            }
        }
    }
}

/// 读一帧（含 4 字节长度前缀）；EOF 返回 `None`。
async fn read_frame(stream: &mut UnixStream) -> std::io::Result<Option<Vec<u8>>> {
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
            "非法帧长度",
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
async fn write_frame(stream: &mut UnixStream, msg: &impl serde::Serialize) -> std::io::Result<()> {
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
    use hmp_player_gst::{LoadRequest, PlayerEvent};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use tokio::sync::{broadcast, watch};

    /// 最小 fake 播放驱动（本模块测试专用）。
    struct SDriver {
        state_tx: watch::Sender<PlaybackState>,
        events_tx: broadcast::Sender<PlayerEvent>,
    }
    impl PlaybackDriver for SDriver {
        fn load(&self, _r: LoadRequest) {}
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
    struct SResolver;
    impl SourceResolver for SResolver {
        fn resolve_source_ids(
            &self,
            _s: &PlayRequest,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<TrackId>, EngineError>> + Send + '_>> {
            Box::pin(async { Ok(vec![TrackId::new("a")]) })
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
                        qualities: vec![],
                    },
                    uri: format!("fake://{id}"),
                    media: None,
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

    async fn temp_socket() -> (PathBuf, UnixListener) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hmp-test.sock");
        let listener = UnixListener::bind(&path).unwrap();
        // TempDir 必须保持存活到测试结束：drop 会删除 socket 文件，
        // 使后续 `UnixStream::connect` 失败（ENOENT）。
        std::mem::forget(dir);
        (path, listener)
    }

    /// 连接 → 发送一帧 → 读一帧响应（每次新建连接）。
    async fn request(sock: &PathBuf, req: &Request) -> Response {
        let mut stream = UnixStream::connect(sock).await.unwrap();
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
        let mut stream = UnixStream::connect(&sock).await.unwrap();
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

    #[tokio::test]
    async fn malformed_frame_gets_bad_request() {
        let (sock, listener) = temp_socket().await;
        let handle = test_engine(true).await;
        tokio::spawn(async move { serve(listener, handle).await });
        let mut stream = UnixStream::connect(&sock).await.unwrap();
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
}
