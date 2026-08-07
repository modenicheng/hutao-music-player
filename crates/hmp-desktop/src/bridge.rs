//! UI 桥接：Slint 回调 ↔ 应用命令 / 事件。

use slint::{ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::app::{AppCommand, AppEvent, UiSongData};

/// 绑定 UI 回调 → 应用命令通道。
pub fn bind_callbacks(
    ui: &crate::AppWindow,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
) {
    let tx = cmd_tx.clone();
    ui.on_search_requested(move |text| {
        let _ = tx.send(AppCommand::Search(text.into()));
    });
    let tx = cmd_tx.clone();
    ui.on_play_requested(move |idx| {
        let _ = tx.send(AppCommand::PlayIndex(idx as usize));
    });
    let tx = cmd_tx.clone();
    ui.on_play_pause(move || {
        let _ = tx.send(AppCommand::TogglePlay);
    });
    let tx = cmd_tx.clone();
    ui.on_next_requested(move || {
        let _ = tx.send(AppCommand::Next);
    });
    let tx = cmd_tx.clone();
    ui.on_prev_requested(move || {
        let _ = tx.send(AppCommand::Previous);
    });
    let tx = cmd_tx.clone();
    ui.on_seek_requested(move |v| {
        let _ = tx.send(AppCommand::Seek(v));
    });
    let tx = cmd_tx.clone();
    ui.on_volume_requested(move |v| {
        let _ = tx.send(AppCommand::SetVolume(v));
    });
    let tx = cmd_tx.clone();
    ui.on_login_start(move || {
        let _ = tx.send(AppCommand::LoginStart);
    });
    ui.on_login_cancel(move || {
        let _ = cmd_tx.send(AppCommand::LoginCancel);
    });
}

/// 把搜索结果映射为 Slint 结构。
pub fn to_ui_song(s: UiSongData) -> crate::UiSong {
    crate::UiSong {
        title: s.title.into(),
        artist: s.artist.into(),
        duration: s.duration.into(),
    }
}

/// 搜索结果 → Slint 模型。
pub fn songs_model(songs: Vec<UiSongData>) -> ModelRc<crate::UiSong> {
    let model: VecModel<crate::UiSong> =
        VecModel::from(songs.into_iter().map(to_ui_song).collect::<Vec<_>>());
    ModelRc::new(model)
}

/// PNG 字节 → Slint Image（RGBA）。
pub fn decode_png(png: &[u8]) -> Result<slint::Image, Box<dyn std::error::Error>> {
    let img = image::load_from_memory(png)?.to_rgba8();
    let (w, h) = img.dimensions();
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Ok(slint::Image::from_rgba8(buffer))
}

/// 应用事件 → UI 更新（AppCore 事件接收任务）。
///
/// 同步实现：内部短暂持有 `Weak::upgrade()` 的组件（不跨 await），
/// 满足 tokio 任务 `Send` 约束。
pub fn handle_event(ui: &slint::Weak<crate::AppWindow>, evt: AppEvent) -> bool {
    let Some(ui) = ui.upgrade() else { return false };
    match evt {
        AppEvent::SearchDone(songs) => {
            ui.set_songs(songs_model(songs));
        }
        AppEvent::SearchFailed(_)
        | AppEvent::QueueUpdated(_)
        | AppEvent::LyricsLoading(_)
        | AppEvent::LyricsLoaded { .. }
        | AppEvent::LyricsFailed { .. } => {
            // Task 1 defines these contracts; later tasks map them to UI state.
        }
        AppEvent::LoginQr(png) => match decode_png(&png) {
            Ok(img) => {
                ui.set_qr_image(img);
                ui.set_show_login(true);
            }
            Err(e) => {
                ui.set_login_status(format!("二维码解码失败: {e}").into());
            }
        },
        AppEvent::LoginStatus(msg) => {
            ui.set_login_status(msg.into());
        }
        AppEvent::LoginDone(name) => {
            ui.set_logged_in(true);
            ui.set_user_name(name.into());
            ui.set_show_login(false);
        }
    }
    true
}
