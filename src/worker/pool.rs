use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::{sleep, timeout};

#[derive(Clone, Debug)]
pub struct WorkerPool {
    accepting: Arc<AtomicBool>,
    max_workers: usize,
    in_flight: Arc<AtomicUsize>,
    idle_notify: Arc<Notify>,
}

impl WorkerPool {
    pub fn new(max_workers: usize) -> Self {
        Self {
            accepting: Arc::new(AtomicBool::new(true)),
            max_workers: max_workers.max(1),
            in_flight: Arc::new(AtomicUsize::new(0)),
            idle_notify: Arc::new(Notify::new()),
        }
    }

    pub fn is_accepting(&self) -> bool {
        self.accepting.load(Ordering::SeqCst)
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    pub fn try_start_job(&self) -> bool {
        if !self.is_accepting() {
            return false;
        }

        loop {
            let current = self.in_flight();
            if current >= self.max_workers {
                return false;
            }
            if self
                .in_flight
                .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
        }
    }

    pub fn finish_job(&self) {
        loop {
            let current = self.in_flight.load(Ordering::SeqCst);
            if current == 0 {
                self.idle_notify.notify_waiters();
                return;
            }
            if self
                .in_flight
                .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                if current <= 1 {
                    self.idle_notify.notify_waiters();
                }
                return;
            }
        }
    }

    pub fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        tracing::info!(
            component = "worker_pool",
            operation = "stop_accepting",
            status = "success",
            in_flight = self.in_flight(),
            "worker pool stopped accepting new jobs"
        );
    }

    pub async fn drain(&self, timeout_duration: Duration) {
        tracing::info!(
            component = "worker_pool",
            operation = "drain",
            status = "started",
            timeout_ms = timeout_duration.as_millis() as u64,
            in_flight = self.in_flight(),
            "draining worker jobs"
        );

        let drain_future = async {
            loop {
                if self.in_flight() == 0 {
                    break;
                }
                tokio::select! {
                    _ = self.idle_notify.notified() => {},
                    _ = sleep(Duration::from_millis(100)) => {},
                }
            }
        };

        match timeout(timeout_duration, drain_future).await {
            Ok(_) => {
                tracing::info!(
                    component = "worker_pool",
                    operation = "drain",
                    status = "success",
                    in_flight = self.in_flight(),
                    "worker pool drain completed"
                );
            }
            Err(_) => {
                tracing::warn!(
                    component = "worker_pool",
                    operation = "drain",
                    status = "timeout",
                    in_flight = self.in_flight(),
                    "worker pool drain timed out"
                );
            }
        }
    }
}
