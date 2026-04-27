use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppState {
    Starting = 0,
    Running = 1,
    ShuttingDown = 2,
    Stopped = 3,
}

#[derive(Clone, Debug)]
pub struct AppStateStore {
    inner: Arc<AtomicU8>,
}

impl AppStateStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AtomicU8::new(AppState::Starting as u8)),
        }
    }

    pub fn set(&self, state: AppState) {
        self.inner.store(state as u8, Ordering::SeqCst);
    }

    pub fn get(&self) -> AppState {
        match self.inner.load(Ordering::SeqCst) {
            0 => AppState::Starting,
            1 => AppState::Running,
            2 => AppState::ShuttingDown,
            3 => AppState::Stopped,
            _ => AppState::Starting,
        }
    }
}
