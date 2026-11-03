//! 播放态视觉冒烟（winit 真 UI）：预置一条在播曲目，供截图复核
//! Equalizer 跳动/底对齐、播放条进度与队列徽章等播放态视觉。
//! `cargo run -p hmp-desktop --example playback_smoke`

use slint::{ComponentHandle, Global, Model};

fn main() -> Result<(), slint::PlatformError> {
    let ui = hmp_desktop::AppWindow::new()?;

    // 装载/回调挂接的 runtime 依赖（本冒烟无 daemon，快照为空 → 零预取任务）
    let runtime =
        std::sync::Arc::new(hmp_desktop::backend::BackendRuntime::new().expect("tokio runtime"));
    hmp_desktop::bridge::load_data(&ui, &runtime);

    {
        let player = hmp_desktop::Player::get(&ui);
        // 队列投影改 QueueRow（带 cover）后，Data.liked 的 TrackRow 需逐行
        // 升格：冒烟只求"有行可渲染"，封面走程序化占位（与队列缺图回退同源）。
        let liked = hmp_desktop::Data::get(&ui).get_liked();
        let queue: Vec<hmp_desktop::QueueRow> = (0..liked.row_count())
            .filter_map(|i| liked.row_data(i))
            .map(|row| hmp_desktop::QueueRow {
                cover: hmp_desktop::covers::cover_image(&format!("album:{}", row.album)),
                mid: row.mid,
                source: row.source,
                title: row.title,
                artists: row.artists,
                artist_mid: row.artist_mid,
                album: row.album,
                album_mid: row.album_mid,
                duration_ms: row.duration_ms,
                quality: row.quality,
            })
            .collect();
        if let Some(first) = queue.first() {
            let first_mid = first.mid.clone();
            player.set_queue(slint::ModelRc::new(slint::VecModel::from(queue)));
            player.set_queue_current_mid(first_mid);
            player.set_playing(true);
        }
        // 抽屉常开：复核头部按钮（清空/关闭）图标渲染
        player.set_queue_visible(true);
    }

    ui.run()
}
