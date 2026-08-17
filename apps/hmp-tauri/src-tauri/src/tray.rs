use tauri::{
    menu::{MenuBuilder, MenuItem, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager, Wry,
};

pub const TRAY_ID: &str = "main-tray";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrayAction {
    ShowHide,
    PlayPause,
    Previous,
    Next,
    Stop,
    Quit,
}

impl TrayAction {
    fn from_id(id: &str) -> Option<Self> {
        match id {
            "show-hide" => Some(Self::ShowHide),
            "play-pause" => Some(Self::PlayPause),
            "previous" => Some(Self::Previous),
            "next" => Some(Self::Next),
            "stop" => Some(Self::Stop),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }
}

pub struct TrayState {
    play_pause: MenuItem<Wry>,
    previous: MenuItem<Wry>,
    next: MenuItem<Wry>,
}

pub fn build(app: &App) -> tauri::Result<()> {
    let show_hide = MenuItemBuilder::with_id("show-hide", "显示/隐藏").build(app)?;
    let play_pause = MenuItemBuilder::with_id("play-pause", "播放").build(app)?;
    let previous = MenuItemBuilder::with_id("previous", "上一首").build(app)?;
    let next = MenuItemBuilder::with_id("next", "下一首").build(app)?;
    let stop = MenuItemBuilder::with_id("stop", "停止").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "完整退出").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&show_hide, &play_pause, &previous, &next, &stop])
        .separator()
        .item(&quit)
        .build()?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("HuTao Music Player")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if let Some(action) = TrayAction::from_id(event.id().as_ref()) {
                dispatch(app, action);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(TrayState {
        play_pause,
        previous,
        next,
    });
    Ok(())
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn update_for_state(app: &AppHandle, state: &crate::control::PlayerStateDto) {
    if let Some(tray) = app.try_state::<TrayState>() {
        let label = if state.status == "playing" {
            "暂停"
        } else {
            "播放"
        };
        let _ = tray.play_pause.set_text(label);
        let _ = tray.previous.set_enabled(state.can_go_previous);
        let _ = tray.next.set_enabled(state.can_go_next);
    }
}

fn dispatch(app: &AppHandle, action: TrayAction) {
    match action {
        TrayAction::ShowHide => toggle_main_window(app),
        TrayAction::Quit => {
            tauri::async_runtime::spawn(crate::lifecycle::complete_exit(app.clone()));
        }
        TrayAction::PlayPause => spawn_player_command(app, hmp_core::PlayerCommand::TogglePlay),
        TrayAction::Previous => spawn_player_command(app, hmp_core::PlayerCommand::Previous),
        TrayAction::Next => spawn_player_command(app, hmp_core::PlayerCommand::Next),
        TrayAction::Stop => spawn_player_command(app, hmp_core::PlayerCommand::Stop),
    }
}

fn toggle_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    match window.is_visible() {
        Ok(true) => {
            let _ = window.hide();
        }
        _ => show_main_window(app),
    }
}

fn spawn_player_command(app: &AppHandle, command: hmp_core::PlayerCommand) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::control::send_player_command(&app, command).await {
            use tauri::Emitter;
            let _ = app.emit(crate::control::CONTROL_ERROR_EVENT, error);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_controller_actions() {
        assert_eq!(TrayAction::from_id("show-hide"), Some(TrayAction::ShowHide));
        assert_eq!(
            TrayAction::from_id("play-pause"),
            Some(TrayAction::PlayPause)
        );
        assert_eq!(TrayAction::from_id("previous"), Some(TrayAction::Previous));
        assert_eq!(TrayAction::from_id("next"), Some(TrayAction::Next));
        assert_eq!(TrayAction::from_id("stop"), Some(TrayAction::Stop));
        assert_eq!(TrayAction::from_id("quit"), Some(TrayAction::Quit));
        assert_eq!(TrayAction::from_id("unknown"), None);
    }
}
