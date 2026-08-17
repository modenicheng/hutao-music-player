use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{Mutex, RwLock};

pub const PLAYER_STATE_EVENT: &str = "hmp://player-state";
pub const CONTROL_ERROR_EVENT: &str = "hmp://control-error";

#[derive(Default)]
pub struct ControlState {
    client: Mutex<Option<hmp_control::ControlClient>>,
    snapshot: RwLock<Option<hmp_core::DaemonState>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStateDto {
    pub status: String,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub volume: f64,
    pub can_seek: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub error: Option<String>,
}

impl From<&hmp_core::DaemonState> for PlayerStateDto {
    fn from(state: &hmp_core::DaemonState) -> Self {
        let status = match state.playback.status {
            hmp_core::PlaybackStatus::Empty => "empty",
            hmp_core::PlaybackStatus::Loading => "loading",
            hmp_core::PlaybackStatus::Buffering => "buffering",
            hmp_core::PlaybackStatus::Playing => "playing",
            hmp_core::PlaybackStatus::Paused => "paused",
            hmp_core::PlaybackStatus::Stopped => "stopped",
            hmp_core::PlaybackStatus::Ended => "ended",
            hmp_core::PlaybackStatus::Error => "error",
        }
        .to_owned();
        let current = state.playback.current.as_ref();
        Self {
            status,
            position_ms: state.playback.position.as_millis() as u64,
            duration_ms: state
                .playback
                .duration
                .map(|duration| duration.as_millis() as u64),
            volume: state.playback.volume,
            can_seek: state.playback.can_seek,
            can_go_next: state.caps.can_go_next,
            can_go_previous: state.caps.can_go_previous,
            title: current.map(|track| track.title.clone()),
            artists: current
                .map(|track| {
                    track
                        .artists
                        .iter()
                        .map(|artist| artist.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            error: state.last_error.as_ref().map(|error| error.message.clone()),
        }
    }
}

impl ControlState {
    pub async fn initialize(app: AppHandle) -> Result<(), String> {
        let client = match hmp_control::ControlClient::connect().await {
            Ok(client) => client,
            Err(_) => {
                spawn_frontend_daemon(&app)?;
                wait_for_client().await?
            }
        };
        let subscription = hmp_control::Subscription::connect(true)
            .await
            .map_err(|error| error.to_string())?;
        let state = app.state::<ControlState>();
        *state.client.lock().await = Some(client);
        tokio::spawn(forward_subscription(app, subscription));
        Ok(())
    }

    async fn request(&self, request: hmp_core::Request) -> Result<hmp_core::Response, String> {
        let mut client = self.client.lock().await;
        let client = client
            .as_mut()
            .ok_or_else(|| "播放后端尚未就绪".to_owned())?;
        client
            .request(request)
            .await
            .map_err(|error| error.to_string())
    }

    async fn command(&self, command: hmp_core::PlayerCommand) -> Result<(), String> {
        expect_ok(self.request(hmp_core::Request::Command(command)).await?)
    }
}

async fn forward_subscription(app: AppHandle, mut subscription: hmp_control::Subscription) {
    loop {
        match subscription.next().await {
            Ok(hmp_control::Event::Engine(hmp_core::Event::StateChanged(snapshot))) => {
                let dto = PlayerStateDto::from(&snapshot);
                *app.state::<ControlState>().snapshot.write().await = Some(snapshot);
                crate::tray::update_for_state(&app, &dto);
                let _ = app.emit(PLAYER_STATE_EVENT, dto);
            }
            Err(error) => {
                let _ = app.emit(CONTROL_ERROR_EVENT, error.to_string());
                break;
            }
        }
    }
}

fn spawn_frontend_daemon(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_shell::ShellExt;

    let (mut events, _child) = app
        .shell()
        .sidecar("hmpd")
        .map_err(|error| error.to_string())?
        .args(["--frontend-owned"])
        .spawn()
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn(async move { while events.recv().await.is_some() {} });
    Ok(())
}

async fn wait_for_client() -> Result<hmp_control::ControlClient, String> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match hmp_control::ControlClient::connect().await {
            Ok(client) => return Ok(client),
            Err(error) if tokio::time::Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(error) => return Err(format!("播放后端启动超时: {error}")),
        }
    }
}

fn expect_ok(response: hmp_core::Response) -> Result<(), String> {
    match response {
        hmp_core::Response::Ok => Ok(()),
        hmp_core::Response::Err { message, .. } => Err(message),
        other => Err(format!("后端返回了意外响应: {other:?}")),
    }
}

#[tauri::command]
pub async fn get_player_state(state: State<'_, ControlState>) -> Result<PlayerStateDto, String> {
    match state.request(hmp_core::Request::Status).await? {
        hmp_core::Response::Status(snapshot) => {
            let dto = PlayerStateDto::from(&snapshot);
            *state.snapshot.write().await = Some(snapshot);
            Ok(dto)
        }
        hmp_core::Response::Err { message, .. } => Err(message),
        response => Err(format!("后端返回了意外响应: {response:?}")),
    }
}

#[tauri::command]
pub async fn toggle_play(state: State<'_, ControlState>) -> Result<(), String> {
    state.command(hmp_core::PlayerCommand::TogglePlay).await
}

#[tauri::command]
pub async fn seek(position_ms: u64, state: State<'_, ControlState>) -> Result<(), String> {
    state
        .command(hmp_core::PlayerCommand::Seek(
            std::time::Duration::from_millis(position_ms),
        ))
        .await
}

#[tauri::command]
pub async fn set_volume(volume: f64, state: State<'_, ControlState>) -> Result<(), String> {
    state
        .command(hmp_core::PlayerCommand::SetVolume(volume.clamp(0.0, 1.0)))
        .await
}

#[tauri::command]
pub async fn previous(state: State<'_, ControlState>) -> Result<(), String> {
    state.command(hmp_core::PlayerCommand::Previous).await
}

#[tauri::command]
pub async fn next(state: State<'_, ControlState>) -> Result<(), String> {
    state.command(hmp_core::PlayerCommand::Next).await
}

#[tauri::command]
pub async fn stop(state: State<'_, ControlState>) -> Result<(), String> {
    state.command(hmp_core::PlayerCommand::Stop).await
}

pub async fn quit_daemon(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<ControlState>();
    expect_ok(state.request(hmp_core::Request::Quit).await?)?;

    loop {
        match hmp_control::ControlClient::connect().await {
            Ok(client) => {
                drop(client);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(_) => return Ok(()),
        }
    }
}

pub async fn send_player_command(
    app: &AppHandle,
    command: hmp_core::PlayerCommand,
) -> Result<(), String> {
    app.state::<ControlState>().command(command).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_state_maps_to_millisecond_frontend_dto() {
        let mut state = hmp_core::DaemonState::default();
        state.playback.status = hmp_core::PlaybackStatus::Playing;
        state.playback.position = std::time::Duration::from_millis(1_250);
        state.playback.duration = Some(std::time::Duration::from_millis(60_500));
        state.playback.volume = 0.4;
        state.playback.can_seek = true;
        state.caps.can_go_next = true;

        let dto = PlayerStateDto::from(&state);

        assert_eq!(dto.status, "playing");
        assert_eq!(dto.position_ms, 1_250);
        assert_eq!(dto.duration_ms, Some(60_500));
        assert_eq!(dto.volume, 0.4);
        assert!(dto.can_seek);
        assert!(dto.can_go_next);
        assert!(!dto.can_go_previous);
    }
}
