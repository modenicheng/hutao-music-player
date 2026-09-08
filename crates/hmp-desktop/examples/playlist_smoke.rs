//! PlayList IPC 真机冒烟（审计重构回归）：本地曲目整表入队 + 起播下标 +
//! 进度推进 + 上一曲能力。daemon 需已运行（`hmp serve`）。
//! `cargo run -p hmp-desktop --example playlist_smoke`

use std::time::Duration;

use hmp_core::TrackId;
use hmp_core::ipc::{DaemonState, Request, Response};

use hmp_desktop::backend::request;

async fn status() -> Option<DaemonState> {
    match request(Request::Status).await {
        Ok(Response::Status(state)) => Some(state),
        other => {
            eprintln!("status 失败: {other:?}");
            None
        }
    }
}

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async move {
        let snap = hmp_desktop::library_view::load_snapshot();
        let ids: Vec<TrackId> = snap
            .local_tracks
            .iter()
            .map(|row| TrackId::new(row.mid.clone()))
            .collect();
        assert!(!ids.is_empty(), "媒体库为空：先 hmp library scan <dir>");
        println!("本地曲目 {} 首，从下标 1 起播", ids.len());

        request(Request::PlayList {
            ids: ids.clone(),
            start: 1,
        })
        .await
        .expect("PlayList 受理失败");
        tokio::time::sleep(Duration::from_millis(1_500)).await;

        let state = status().await.expect("取状态失败");
        assert_eq!(state.queue.len, ids.len(), "整表入队");
        assert_eq!(state.queue.current, Some(1), "起播下标生效");
        assert!(state.caps.can_go_previous, "多曲队列上一曲可用");
        assert_eq!(
            state.playback.status,
            hmp_core::PlaybackStatus::Playing,
            "起播曲应为播放态"
        );
        let pos1 = state.playback.position;
        assert!(pos1 > Duration::ZERO, "起播曲进度非零（音频管线在走）");
        println!("当前曲: {:?} @ {:?}", state.playback.current.map(|t| t.title), pos1);

        tokio::time::sleep(Duration::from_secs(3)).await;
        let state2 = status().await.expect("取状态失败");
        assert!(
            state2.playback.position > pos1,
            "3s 后进度应前进（设备输出正常）: {:?} → {:?}",
            pos1,
            state2.playback.position
        );
        println!(
            "进度推进: {:?} → {:?}，PlayList 真机冒烟通过",
            pos1, state2.playback.position
        );
    });
}
