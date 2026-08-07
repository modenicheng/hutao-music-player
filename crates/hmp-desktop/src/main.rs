//! HMP 桌面应用入口：Slint UI + 应用核心编排（docs/PROJECT.md §4.1）。

use hmp_desktop::{AppCore, AppWindow, UiPlayback, app, bridge, demo};
use slint::ComponentHandle;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let _guard = runtime.enter();

    // 命令 / 事件通道
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();

    let mut core = AppCore::new(cmd_rx, event_tx)?;
    let ui = AppWindow::new()?;

    // UI 回调 → 应用命令 / 本地界面状态
    bridge::bind_callbacks(&ui, cmd_tx);
    bridge::bind_ui_state_callbacks(&ui);
    let weak = ui.as_weak();
    ui.on_queue_requested(move || {
        if let Some(ui) = weak.upgrade() {
            ui.invoke_navigate_requested("queue".into());
        }
    });
    let weak = ui.as_weak();
    ui.on_lyrics_requested(move || {
        if let Some(ui) = weak.upgrade() {
            ui.invoke_navigate_requested("lyrics".into());
        }
    });

    // Root state defaults are explicit so startup does not depend on generated properties.
    ui.set_current_page("library".into());
    ui.set_theme_mode("system".into());
    ui.set_logged_in(core.logged_in());
    ui.set_user_name(core.user_name().into());
    ui.set_login_status("".into());
    ui.set_playback(playback_default());
    ui.set_current_track_id("".into());
    ui.set_library_items(bridge::library_model(Vec::new()));
    ui.set_recommend_items(bridge::library_model(demo::demo_recommendations()));
    ui.set_feature_statuses(bridge::feature_model(demo::feature_matrix()));
    ui.set_songs(bridge::songs_model(Vec::new()));
    ui.set_search_text("".into());
    ui.set_search_query_valid(false);
    ui.set_search_loading(false);
    ui.set_search_error_text("".into());
    ui.set_queue(bridge::queue_model(Vec::new()));
    ui.set_lyrics(bridge::lyrics_model(Vec::new(), 0.0));
    ui.set_lyrics_state("idle".into());
    ui.set_lyrics_request_mid("".into());
    ui.set_lyrics_error_text("".into());
    // Publish the real initial queue before AppCore is moved into its task.
    ui.set_queue(bridge::queue_model(core.queue_snapshot()));

    // 播放状态订阅（core 随后 move 进事件循环）
    let state_rx = core.player.subscribe_state();

    // 应用核心事件循环
    runtime.spawn(async move { core.run().await });

    // AppCore 事件 → UI
    let event_ui = ui.as_weak();
    runtime.spawn(async move {
        let mut event_rx = event_rx;
        while let Some(evt) = event_rx.recv().await {
            if !bridge::handle_event(&event_ui, evt) {
                break;
            }
        }
    });

    // 播放状态 → UI
    let state_ui = ui.as_weak();
    runtime.spawn(async move {
        let mut state_rx = state_rx;
        loop {
            if state_rx.changed().await.is_err() {
                break;
            }
            let s = state_rx.borrow().clone();
            let Some(ui) = state_ui.upgrade() else { break };
            let (title, artist, status, pos, dur, pos_text, dur_text) = app::playback_snapshot(&s);
            ui.set_playback(UiPlayback {
                title: title.into(),
                artist: artist.into(),
                status: status.into(),
                position: pos,
                duration: dur,
                volume: s.volume as f32,
                position_text: pos_text.into(),
                duration_text: dur_text.into(),
            });
            ui.set_current_track_id(
                s.current
                    .as_ref()
                    .map(|track| track.id.to_string())
                    .unwrap_or_default()
                    .into(),
            );
            bridge::update_lyrics_active_line(&ui.get_lyrics(), s.position.as_secs_f32() * 1000.0);
        }
    });

    ui.run()?;
    Ok(())
}

/// 默认播放状态。
fn playback_default() -> UiPlayback {
    UiPlayback {
        title: "".into(),
        artist: "".into(),
        status: "stopped".into(),
        position: 0.0,
        duration: 0.0,
        volume: 1.0,
        position_text: "00:00".into(),
        duration_text: "00:00".into(),
    }
}
