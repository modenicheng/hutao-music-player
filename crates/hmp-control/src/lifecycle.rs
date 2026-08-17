use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleMode {
    Autonomous,
    FrontendOwned { orphan_grace: Duration },
}

#[derive(Clone)]
pub struct FrontendLeaseTracker {
    inner: Arc<Inner>,
}

struct Inner {
    mode: LifecycleMode,
    state: Mutex<LeaseState>,
    quit_tx: mpsc::UnboundedSender<hmp_core::Request>,
}

#[derive(Default)]
struct LeaseState {
    active: usize,
    generation: u64,
}

impl FrontendLeaseTracker {
    pub fn frontend_owned(
        orphan_grace: Duration,
        quit_tx: mpsc::UnboundedSender<hmp_core::Request>,
    ) -> Self {
        Self::new(LifecycleMode::FrontendOwned { orphan_grace }, quit_tx)
    }

    pub fn autonomous(quit_tx: mpsc::UnboundedSender<hmp_core::Request>) -> Self {
        Self::new(LifecycleMode::Autonomous, quit_tx)
    }

    pub fn new(mode: LifecycleMode, quit_tx: mpsc::UnboundedSender<hmp_core::Request>) -> Self {
        let tracker = Self {
            inner: Arc::new(Inner {
                mode,
                state: Mutex::new(LeaseState::default()),
                quit_tx,
            }),
        };
        if let LifecycleMode::FrontendOwned { orphan_grace } = mode {
            schedule_orphan_shutdown(Arc::clone(&tracker.inner), 0, orphan_grace);
        }
        tracker
    }

    pub fn acquire(&self) -> FrontendLeaseGuard {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("frontend lease lock poisoned");
        state.active += 1;
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        FrontendLeaseGuard {
            inner: Arc::clone(&self.inner),
            active: true,
        }
    }

    pub fn active_count(&self) -> usize {
        self.inner
            .state
            .lock()
            .expect("frontend lease lock poisoned")
            .active
    }
}

pub struct FrontendLeaseGuard {
    inner: Arc<Inner>,
    active: bool,
}

impl Drop for FrontendLeaseGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let (generation, became_orphaned) = {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("frontend lease lock poisoned");
            state.active = state.active.saturating_sub(1);
            state.generation = state.generation.wrapping_add(1);
            (state.generation, state.active == 0)
        };
        let LifecycleMode::FrontendOwned { orphan_grace } = self.inner.mode else {
            return;
        };
        if !became_orphaned {
            return;
        }
        schedule_orphan_shutdown(Arc::clone(&self.inner), generation, orphan_grace);
    }
}

fn schedule_orphan_shutdown(inner: Arc<Inner>, generation: u64, orphan_grace: Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(orphan_grace).await;
        let should_quit = {
            let state = inner.state.lock().expect("frontend lease lock poisoned");
            state.active == 0 && state.generation == generation
        };
        if should_quit {
            let _ = inner.quit_tx.send(hmp_core::Request::Quit);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn frontend_owned_daemon_quits_after_orphan_grace() {
        let (quit_tx, mut quit_rx) = mpsc::unbounded_channel();
        let lifecycle = FrontendLeaseTracker::frontend_owned(Duration::from_secs(30), quit_tx);
        let lease = lifecycle.acquire();
        drop(lease);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(29)).await;
        assert!(quit_rx.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(quit_rx.try_recv().unwrap(), hmp_core::Request::Quit);
    }

    #[tokio::test(start_paused = true)]
    async fn frontend_owned_daemon_without_first_lease_quits_after_grace() {
        let (quit_tx, mut quit_rx) = mpsc::unbounded_channel();
        let _lifecycle =
            FrontendLeaseTracker::frontend_owned(Duration::from_secs(30), quit_tx);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(29)).await;
        assert!(quit_rx.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(quit_rx.try_recv().unwrap(), hmp_core::Request::Quit);
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_cancels_orphan_shutdown() {
        let (quit_tx, mut quit_rx) = mpsc::unbounded_channel();
        let lifecycle = FrontendLeaseTracker::frontend_owned(Duration::from_secs(30), quit_tx);
        let first = lifecycle.acquire();
        drop(first);
        tokio::time::advance(Duration::from_secs(20)).await;
        let _replacement = lifecycle.acquire();
        tokio::time::advance(Duration::from_secs(20)).await;
        assert!(quit_rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn autonomous_daemon_ignores_frontend_disconnect() {
        let (quit_tx, mut quit_rx) = mpsc::unbounded_channel();
        let lifecycle = FrontendLeaseTracker::autonomous(quit_tx);
        drop(lifecycle.acquire());
        tokio::time::advance(Duration::from_secs(300)).await;
        assert!(quit_rx.try_recv().is_err());
    }

    #[test]
    fn autonomous_guard_still_releases_its_active_count() {
        let (quit_tx, _quit_rx) = mpsc::unbounded_channel();
        let lifecycle = FrontendLeaseTracker::autonomous(quit_tx);
        let lease = lifecycle.acquire();
        assert_eq!(lifecycle.active_count(), 1);
        drop(lease);
        assert_eq!(lifecycle.active_count(), 0);
    }
}
