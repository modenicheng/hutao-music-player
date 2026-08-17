mod control;
mod lifecycle;
mod tray;

use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Plugins run in registration order. The single-instance guard must be first.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .manage(control::ControlState::default())
        .manage(lifecycle::Lifecycle::default())
        .setup(|app| {
            tray::build(app)?;
            app.state::<lifecycle::Lifecycle>().mark_ready(true);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = control::ControlState::initialize(handle.clone()).await {
                    let _ = handle.emit(control::CONTROL_ERROR_EVENT, error);
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle();
                match app.state::<lifecycle::Lifecycle>().on_close_requested() {
                    lifecycle::CloseAction::Hide => {
                        let _ = window.hide();
                    }
                    lifecycle::CloseAction::Quit => {
                        tauri::async_runtime::spawn(lifecycle::complete_exit(app.clone()));
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            control::get_player_state,
            control::toggle_play,
            control::seek,
            control::set_volume,
            control::previous,
            control::next,
            control::stop,
            lifecycle::quit_application,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
