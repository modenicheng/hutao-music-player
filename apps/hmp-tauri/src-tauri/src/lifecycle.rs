use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use tauri::{AppHandle, Manager};

const BOOTING: u8 = 0;
const READY: u8 = 1;
const QUITTING: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    Hide,
    Quit,
}

pub struct Lifecycle {
    phase: AtomicU8,
    tray_ready: AtomicBool,
}

impl Lifecycle {
    pub fn booting() -> Self {
        Self {
            phase: AtomicU8::new(BOOTING),
            tray_ready: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    fn ready(tray_ready: bool) -> Self {
        Self {
            phase: AtomicU8::new(READY),
            tray_ready: AtomicBool::new(tray_ready),
        }
    }

    pub fn mark_ready(&self, tray_ready: bool) {
        self.tray_ready.store(tray_ready, Ordering::Release);
        self.phase.store(READY, Ordering::Release);
    }

    pub fn on_close_requested(&self) -> CloseAction {
        if self.phase.load(Ordering::Acquire) == READY && self.tray_ready.load(Ordering::Acquire) {
            CloseAction::Hide
        } else {
            CloseAction::Quit
        }
    }

    pub fn begin_quit(&self) -> bool {
        self.phase.swap(QUITTING, Ordering::AcqRel) != QUITTING
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::booting()
    }
}

pub async fn complete_exit(app: AppHandle) {
    if !app.state::<Lifecycle>().begin_quit() {
        return;
    }

    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        crate::control::quit_daemon(&app),
    )
    .await;
    let _ = app.remove_tray_by_id(crate::tray::TRAY_ID);
    app.exit(0);
}

#[tauri::command]
pub async fn quit_application(app: AppHandle) {
    complete_exit(app).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_hides_only_when_tray_is_ready() {
        assert_eq!(
            Lifecycle::ready(true).on_close_requested(),
            CloseAction::Hide
        );
        assert_eq!(
            Lifecycle::ready(false).on_close_requested(),
            CloseAction::Quit
        );
    }

    #[test]
    fn complete_exit_is_idempotent() {
        let lifecycle = Lifecycle::ready(true);
        assert!(lifecycle.begin_quit());
        assert!(!lifecycle.begin_quit());
    }
}
