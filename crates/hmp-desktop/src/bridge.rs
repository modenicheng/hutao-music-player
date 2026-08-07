//! UI 桥接：Slint 回调 ↔ 应用命令 / 事件。

use slint::{ComponentHandle, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::app::{AppCommand, AppEvent, ThemeMode, UiLyricData, UiPage, UiQueueData, UiSongData};

/// 绑定 UI 回调 → 应用命令通道。
pub fn bind_callbacks(
    ui: &crate::AppWindow,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
) {
    let weak = ui.as_weak();
    ui.on_search_query_edited(move |text| {
        if let Some(ui) = weak.upgrade() {
            ui.set_search_query_valid(!text.trim().is_empty());
        }
    });

    let weak = ui.as_weak();
    let tx = cmd_tx.clone();
    ui.on_search_requested(move |text| {
        let query = text.trim();
        if query.is_empty() {
            return;
        }
        let Some(ui) = weak.upgrade() else {
            return;
        };
        ui.set_search_loading(true);
        ui.set_search_completed(false);
        ui.set_search_error_text("".into());
        let _ = tx.send(AppCommand::Search(query.to_owned()));
    });
    let weak = ui.as_weak();
    let tx = cmd_tx.clone();
    ui.on_play_requested(move |idx| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(index) = valid_model_index(idx, ui.get_songs().row_count()) else {
            return;
        };
        let _ = tx.send(AppCommand::PlayIndex(index));
    });
    let weak = ui.as_weak();
    let tx = cmd_tx.clone();
    ui.on_play_queue_requested(move |idx| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(index) = valid_model_index(idx, ui.get_queue().row_count()) else {
            return;
        };
        let _ = tx.send(AppCommand::PlayQueueIndex(index));
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
    let tx = cmd_tx.clone();
    ui.on_login_cancel(move || {
        let _ = tx.send(AppCommand::LoginCancel);
    });
    let weak = ui.as_weak();
    let tx = cmd_tx.clone();
    ui.on_load_lyrics_requested(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_current_track_id().trim().is_empty() {
            return;
        }
        let _ = tx.send(AppCommand::ReloadLyrics);
    });
}

pub(crate) fn valid_model_index(index: i32, row_count: usize) -> Option<usize> {
    let index = usize::try_from(index).ok()?;
    (index < row_count).then_some(index)
}

/// 绑定仅影响本地 UI 状态的回调。
pub fn bind_ui_state_callbacks(ui: &crate::AppWindow) {
    let weak = ui.as_weak();
    ui.on_navigate_requested(move |value| {
        let Some(page) = UiPage::parse(value.as_str()) else {
            return;
        };
        if let Some(ui) = weak.upgrade() {
            ui.set_current_page(page.as_str().into());
        }
    });

    let weak = ui.as_weak();
    ui.on_theme_requested(move |value| {
        let Some(mode) = ThemeMode::parse(value.as_str()) else {
            return;
        };
        if let Some(ui) = weak.upgrade() {
            ui.set_theme_mode(mode.as_str().into());
        }
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

pub fn queue_model(items: Vec<UiQueueData>) -> ModelRc<crate::UiQueue> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(|item| crate::UiQueue {
                track_id: item.track_id.into(),
                title: item.title.into(),
                artist: item.artist.into(),
                duration: item.duration.into(),
                is_current: item.is_current,
                is_playing: item.is_playing,
            })
            .collect::<Vec<_>>(),
    ))
}

fn to_ui_lyric(line: UiLyricData, active: bool) -> crate::UiLyric {
    crate::UiLyric {
        time: line.time.into(),
        timestamp_ms: line.timestamp_ms as f32,
        text: line.text.into(),
        translation: line.translation.into(),
        is_active: active,
    }
}

pub fn lyrics_model(lines: Vec<UiLyricData>, position_ms: f32) -> ModelRc<crate::UiLyric> {
    let active_index = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.timestamp_ms as f32 <= position_ms)
        .map(|(index, _)| index)
        .last();
    ModelRc::new(VecModel::from(
        lines
            .into_iter()
            .enumerate()
            .map(|(index, line)| to_ui_lyric(line, Some(index) == active_index))
            .collect::<Vec<_>>(),
    ))
}

pub fn update_lyrics_active_line(model: &ModelRc<crate::UiLyric>, position_ms: f32) {
    let active_index = (0..model.row_count())
        .filter_map(|index| model.row_data(index).map(|line| (index, line)))
        .filter(|(_, line)| line.timestamp_ms <= position_ms)
        .map(|(index, _)| index)
        .last();
    for index in 0..model.row_count() {
        let Some(mut line) = model.row_data(index) else {
            continue;
        };
        line.is_active = Some(index) == active_index;
        model.set_row_data(index, line);
    }
}

pub fn lyrics_model_at_position(
    model: &ModelRc<crate::UiLyric>,
    position_ms: f32,
) -> ModelRc<crate::UiLyric> {
    let updated = ModelRc::new(VecModel::from(
        (0..model.row_count())
            .filter_map(|index| model.row_data(index))
            .collect::<Vec<_>>(),
    ));
    update_lyrics_active_line(&updated, position_ms);
    updated
}

pub(crate) fn lyric_mid_matches(request_mid: &str, event_mid: &str) -> bool {
    !request_mid.trim().is_empty() && !event_mid.trim().is_empty() && request_mid == event_mid
}

/// Library/recommendation data -> Slint model.
pub fn library_model(items: Vec<crate::demo::UiLibraryData>) -> ModelRc<crate::UiLibrary> {
    let model = VecModel::from(
        items
            .into_iter()
            .map(|item| crate::UiLibrary {
                kind: item.kind.into(),
                title: item.title.into(),
                subtitle: item.subtitle.into(),
                status: item.status.into(),
                cover: item.cover,
            })
            .collect::<Vec<_>>(),
    );
    ModelRc::new(model)
}

/// Feature status data -> Slint model.
pub fn feature_model(items: Vec<crate::UiFeatureData>) -> ModelRc<crate::UiFeature> {
    let model = VecModel::from(
        items
            .into_iter()
            .map(|item| crate::UiFeature {
                name: item.name.into(),
                status: item.status.into(),
                detail: item.detail.into(),
            })
            .collect::<Vec<_>>(),
    );
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
    let position_ms = ui.get_playback().position.max(0.0) * 1000.0;
    match evt {
        AppEvent::SearchDone(songs) => {
            ui.set_songs(songs_model(songs));
            ui.set_search_loading(false);
            ui.set_search_completed(true);
            ui.set_search_error_text("".into());
        }
        AppEvent::SearchFailed(message) => {
            ui.set_search_loading(false);
            ui.set_search_completed(true);
            ui.set_search_error_text(message.into());
        }
        AppEvent::QueueUpdated(items) => {
            ui.set_queue(queue_model(items));
        }
        AppEvent::LyricsLoading(mid) => {
            if mid.trim().is_empty() {
                return true;
            }
            ui.set_lyrics_request_mid(mid.into());
            ui.set_lyrics_state("loading".into());
            ui.set_lyrics_error_text("".into());
            ui.set_lyrics(lyrics_model(Vec::new(), 0.0));
        }
        AppEvent::LyricsLoaded { mid, lines } => {
            if !lyric_mid_matches(ui.get_lyrics_request_mid().as_str(), &mid) {
                return true;
            }
            ui.set_lyrics(lyrics_model(lines, position_ms));
            ui.set_lyrics_error_text("".into());
            ui.set_lyrics_state(
                if ui.get_lyrics().row_count() == 0 {
                    "empty"
                } else {
                    "ready"
                }
                .into(),
            );
        }
        AppEvent::LyricsFailed { mid, message } => {
            if !lyric_mid_matches(ui.get_lyrics_request_mid().as_str(), &mid) {
                return true;
            }
            ui.set_lyrics_state("error".into());
            ui.set_lyrics_error_text(message.into());
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
