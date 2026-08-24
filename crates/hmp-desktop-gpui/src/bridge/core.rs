use hmp_core::PlaybackState;
use hmp_desktop_common::{AppCommand, AppCore, AppEvent};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

#[derive(Clone)]
pub struct CoreCommandSender {
    tx: mpsc::UnboundedSender<AppCommand>,
}

impl CoreCommandSender {
    pub fn new(tx: mpsc::UnboundedSender<AppCommand>) -> Self {
        Self { tx }
    }

    pub fn send(&self, command: AppCommand) {
        let _ = self.tx.send(command);
    }

    pub fn search(&self, query: String) {
        self.send(AppCommand::Search(query));
    }

    pub fn play_search_result(&self, index: usize) {
        self.send(AppCommand::PlayIndex(index));
    }

    pub fn play_queue_item(&self, index: usize) {
        self.send(AppCommand::PlayQueueIndex(index));
    }

    pub fn toggle_play(&self) {
        self.send(AppCommand::TogglePlay);
    }

    pub fn next(&self) {
        self.send(AppCommand::Next);
    }

    pub fn previous(&self) {
        self.send(AppCommand::Previous);
    }

    pub fn seek(&self, seconds: f32) {
        self.send(AppCommand::Seek(seconds));
    }

    pub fn set_volume(&self, volume: f32) {
        self.send(AppCommand::SetVolume(volume));
    }
}

pub struct CoreBridge {
    _runtime: Runtime,
    commands: CoreCommandSender,
    playback_rx: watch::Receiver<PlaybackState>,
    event_rx: Option<mpsc::UnboundedReceiver<AppEvent>>,
}

impl CoreBridge {
    pub fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let guard = runtime.enter();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let mut core = AppCore::new(command_rx, event_tx)?;
        let playback_rx = core.player.subscribe_state();
        runtime.spawn(async move { core.run().await });
        drop(guard);

        Ok(Self {
            _runtime: runtime,
            commands: CoreCommandSender::new(command_tx),
            playback_rx,
            event_rx: Some(event_rx),
        })
    }

    pub fn commands(&self) -> CoreCommandSender {
        self.commands.clone()
    }

    pub fn playback_receiver(&self) -> watch::Receiver<PlaybackState> {
        self.playback_rx.clone()
    }

    pub fn take_event_receiver(&mut self) -> mpsc::UnboundedReceiver<AppEvent> {
        self.event_rx
            .take()
            .expect("AppEvent receiver may only be taken once")
    }
}

impl Drop for CoreBridge {
    fn drop(&mut self) {
        self.commands.send(AppCommand::Quit);
    }
}

#[cfg(test)]
mod tests {
    use hmp_desktop_common::AppCommand;
    use tokio::sync::mpsc;

    use super::CoreCommandSender;

    #[test]
    fn playback_controls_emit_existing_app_commands() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let commands = CoreCommandSender::new(tx);

        commands.toggle_play();
        commands.next();
        commands.previous();
        commands.seek(42.5);
        commands.set_volume(0.35);

        assert!(matches!(rx.try_recv().unwrap(), AppCommand::TogglePlay));
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::Next));
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::Previous));
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::Seek(v) if v == 42.5));
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::SetVolume(v) if v == 0.35));
    }

    #[test]
    fn search_and_result_selection_emit_existing_app_commands() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let commands = CoreCommandSender::new(tx);

        commands.search("胡桃".into());
        commands.play_search_result(3);
        commands.play_queue_item(2);

        assert!(matches!(rx.try_recv().unwrap(), AppCommand::Search(query) if query == "胡桃"));
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::PlayIndex(3)));
        assert!(matches!(
            rx.try_recv().unwrap(),
            AppCommand::PlayQueueIndex(2)
        ));
    }
}
