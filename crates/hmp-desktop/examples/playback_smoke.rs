//! 播放态视觉冒烟（winit 真 UI）：预置一条在播曲目，供截图复核
//! Equalizer 跳动/底对齐、播放条进度与队列徽章等播放态视觉。
//! `cargo run -p hmp-desktop --example playback_smoke`

use slint::{ComponentHandle, Global, Model};

fn main() -> Result<(), slint::PlatformError> {
    let ui = hmp_desktop::AppWindow::new()?;

    hmp_desktop::bridge::load_data(&ui);

    {
        let player = hmp_desktop::Player::get(&ui);
        let liked = hmp_desktop::Data::get(&ui).get_liked();
        if let Some(first) = (0..liked.row_count()).find_map(|i| liked.row_data(i)) {
            player.set_queue(liked);
            player.set_queue_current_mid(first.mid);
            player.set_playing(true);
        }
        // 抽屉常开：复核头部按钮（清空/关闭）图标渲染
        player.set_queue_visible(true);
    }

    ui.run()
}
