//! 各遥控子命令（spec §4.3）。

use std::io::Write;

use hmp_core::ipc::{IpcErrorCode, Request, Response};
use hmp_core::{DaemonState, PlayerCommand};

use crate::client::{CliError, DaemonClient};

/// 格式化状态为人类可读文本。
pub fn format_status(st: &DaemonState) -> String {
    let mut s = String::new();
    let track = st.playback.current.as_ref();
    let title = track.map(|t| t.title.as_str()).unwrap_or("（无）");
    let artist = track
        .map(|t| {
            t.artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_default();
    s.push_str(&format!("状态: {:?}\n", st.playback.status));
    s.push_str(&format!("曲目: {title} - {artist}\n"));
    match st.playback.duration {
        Some(d) => s.push_str(&format!(
            "进度: {} / {}\n",
            fmt_duration(st.playback.position),
            fmt_duration(d)
        )),
        None => s.push_str(&format!("进度: {}\n", fmt_duration(st.playback.position))),
    }
    s.push_str(&format!("音量: {:.0}%\n", st.playback.volume * 100.0));
    s.push_str(&format!(
        "循环: {:?}  随机: {}\n",
        st.playback.loop_mode, st.playback.shuffle
    ));
    s.push_str(&format!("队列: {} 首\n", st.queue.tracks.len()));
    s
}

fn fmt_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// 通用：发命令并打印响应错误。
async fn send(client: &mut DaemonClient, req: Request) -> Result<Response, CliError> {
    client.request(&req).await
}

/// `hmp play <track-id|playlist:xxx|album:xxx>`（前缀识别源类型）。
pub async fn cmd_play(client: &mut DaemonClient, src: &str) -> Result<(), CliError> {
    let req = Request::Play(parse_source(src));
    let resp = send(client, req).await?;
    match resp {
        Response::Ok => await_playing(client).await,
        Response::Err { code, message } => Err(CliError::Response { code, message }),
        _ => Err(CliError::Protocol("意外响应".into())),
    }
}

/// `hmp playnext <track-id|playlist:xxx|album:xxx>`（插队并立即播放）。
pub async fn cmd_playnext(client: &mut DaemonClient, src: &str) -> Result<(), CliError> {
    let req = Request::PlayNext(parse_source(src));
    let resp = send(client, req).await?;
    match resp {
        Response::Ok => await_playing(client).await,
        Response::Err { code, message } => Err(CliError::Response { code, message }),
        _ => Err(CliError::Protocol("意外响应".into())),
    }
}

/// 短轮询确认（≤15s）：等到非 Loading。
async fn await_playing(client: &mut DaemonClient) -> Result<(), CliError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let st = match send(client, Request::Status).await? {
            Response::Status(s) => s,
            _ => return Err(CliError::Protocol("Status 响应异常".into())),
        };
        use hmp_core::PlaybackStatus as S;
        match st.playback.status {
            S::Playing | S::Paused => {
                let mut out = std::io::stdout().lock();
                writeln!(
                    out,
                    "已开始播放: {}",
                    st.playback
                        .current
                        .as_ref()
                        .map(|t| t.title.as_str())
                        .unwrap_or("?")
                )?;
                out.flush()?;
                return Ok(());
            }
            S::Error => {
                return Err(CliError::Response {
                    code: IpcErrorCode::Internal,
                    message: "播放失败（见后端日志）".into(),
                });
            }
            S::Empty => {
                return Err(CliError::Response {
                    code: IpcErrorCode::Internal,
                    message: "后端空闲，播放未启动".into(),
                });
            }
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::Response {
                code: IpcErrorCode::Internal,
                message: "播放确认超时（15s）".into(),
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}

/// 解析播放源：`playlist:<id>` / `album:<id>` / 其他 = 单曲。
pub fn parse_source(src: &str) -> hmp_core::PlayRequest {
    if let Some(id) = src.strip_prefix("playlist:") {
        hmp_core::PlayRequest::Playlist(hmp_core::PlaylistId::new(id))
    } else if let Some(id) = src.strip_prefix("album:") {
        hmp_core::PlayRequest::Album(hmp_core::AlbumId::new(id))
    } else {
        hmp_core::PlayRequest::Track(hmp_core::TrackId::new(src))
    }
}

/// `hmp status`。
pub async fn cmd_status(client: &mut DaemonClient) -> Result<(), CliError> {
    let resp = send(client, Request::Status).await?;
    match resp {
        Response::Status(st) => {
            let mut out = std::io::stdout().lock();
            write!(out, "{}", format_status(&st))?;
            out.flush()?;
            Ok(())
        }
        _ => Err(CliError::Protocol("Status 响应异常".into())),
    }
}

/// 简单命令（Pause/Resume/Next/Prev/Stop/Quit 等）通用执行。
pub async fn cmd_simple(client: &mut DaemonClient, req: Request) -> Result<(), CliError> {
    let resp = send(client, req).await?;
    match resp {
        Response::Ok => Ok(()),
        Response::Err { code, message } => Err(CliError::Response { code, message }),
        _ => Err(CliError::Protocol("意外响应".into())),
    }
}

/// `hmp queue show|add <id>|remove <idx>|clear`。
pub async fn cmd_queue(client: &mut DaemonClient, args: &[String]) -> Result<(), CliError> {
    match args.first().map(|s| s.as_str()) {
        None | Some("show") => {
            let resp = send(client, Request::Queue).await?;
            if let Response::Queue(q) = resp {
                let mut out = std::io::stdout().lock();
                for (i, t) in q.tracks.iter().enumerate() {
                    let mark = if Some(i) == q.current { "▶" } else { " " };
                    writeln!(out, "{mark} {i}: {t}")?;
                }
                out.flush()?;
                return Ok(());
            }
            Err(CliError::Protocol("Queue 响应异常".into()))
        }
        Some("add") => {
            let id = args.get(1).ok_or_else(|| CliError::Response {
                code: IpcErrorCode::BadRequest,
                message: "queue add 需要曲目 id".into(),
            })?;
            cmd_simple(client, Request::QueueAppend(parse_source(id))).await
        }
        Some("remove") => {
            let idx: usize =
                args.get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| CliError::Response {
                        code: IpcErrorCode::BadRequest,
                        message: "queue remove 需要 0 基索引".into(),
                    })?;
            cmd_simple(client, Request::QueueRemove(idx)).await
        }
        Some("clear") => cmd_simple(client, Request::QueueClear).await,
        _ => Err(CliError::Response {
            code: IpcErrorCode::BadRequest,
            message: "未知 queue 子命令".into(),
        }),
    }
}

/// 便捷构造。
pub fn pause_req() -> Request {
    Request::Command(PlayerCommand::Pause)
}
pub fn resume_req() -> Request {
    Request::Command(PlayerCommand::Play)
}
pub fn next_req() -> Request {
    Request::Command(PlayerCommand::Next)
}
pub fn prev_req() -> Request {
    Request::Command(PlayerCommand::Previous)
}
pub fn stop_req() -> Request {
    Request::Command(PlayerCommand::Stop)
}
pub fn seek_req(secs: u64) -> Request {
    Request::Command(PlayerCommand::Seek(std::time::Duration::from_secs(secs)))
}
pub fn volume_req(v: f64) -> Request {
    Request::Command(PlayerCommand::SetVolume(v.clamp(0.0, 1.0)))
}
pub fn loop_req(m: hmp_core::LoopMode) -> Request {
    Request::Command(PlayerCommand::SetLoopMode(m))
}
pub fn shuffle_req(b: bool) -> Request {
    Request::Command(PlayerCommand::SetShuffle(b))
}
pub fn quit_req() -> Request {
    Request::Quit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_output_includes_track_and_status() {
        let st = hmp_core::DaemonState {
            playback: hmp_core::PlaybackState {
                status: hmp_core::PlaybackStatus::Playing,
                current: Some(hmp_core::Track {
                    id: hmp_core::TrackId::new("m1"),
                    title: "稻香".into(),
                    artists: vec![hmp_core::ArtistRef {
                        id: hmp_core::ArtistId::new("a1"),
                        name: "周杰伦".into(),
                    }],
                    album: None,
                    duration: Some(std::time::Duration::from_secs(300)),
                    cover: None,
                    url: Some("fake://m1".into()),
                    qualities: vec![],
                }),
                position: std::time::Duration::from_secs(30),
                duration: Some(std::time::Duration::from_secs(300)),
                ..Default::default()
            },
            queue: Default::default(),
            caps: Default::default(),
        };
        let s = format_status(&st);
        assert!(s.contains("稻香"));
        assert!(s.contains("Playing"));
        assert!(s.contains("00:30 / 05:00"));
    }

    #[test]
    fn parse_source_detects_prefixes() {
        assert_eq!(
            parse_source("m1"),
            hmp_core::PlayRequest::Track(hmp_core::TrackId::new("m1"))
        );
        assert_eq!(
            parse_source("playlist:p1"),
            hmp_core::PlayRequest::Playlist(hmp_core::PlaylistId::new("p1"))
        );
        assert_eq!(
            parse_source("album:a1"),
            hmp_core::PlayRequest::Album(hmp_core::AlbumId::new("a1"))
        );
    }

    #[test]
    fn constructors_build_expected_requests() {
        assert_eq!(pause_req(), Request::Command(PlayerCommand::Pause));
        assert_eq!(resume_req(), Request::Command(PlayerCommand::Play));
        assert_eq!(
            seek_req(90),
            Request::Command(PlayerCommand::Seek(std::time::Duration::from_secs(90)))
        );
        assert_eq!(
            volume_req(1.5),
            Request::Command(PlayerCommand::SetVolume(1.0)) // clamp 到 0..1
        );
        assert_eq!(quit_req(), Request::Quit);
    }
}
