use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct HttpHealthServer {
    accepting: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
    drained_notify: Arc<Notify>,
}

impl HttpHealthServer {
    pub fn new() -> Self {
        Self {
            accepting: Arc::new(AtomicBool::new(true)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            drained_notify: Arc::new(Notify::new()),
        }
    }

    pub fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        tracing::info!(
            component = "http_transport",
            operation = "stop_accepting",
            status = "success",
            in_flight = self.in_flight.load(Ordering::SeqCst),
            "http transport stopped accepting requests"
        );
    }

    pub async fn run(&self, shutdown: CancellationToken) {
        tracing::info!(
            component = "http_transport",
            operation = "start",
            status = "success",
            "http transport started"
        );

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    self.stop_accepting();
                    break;
                }
                _ = sleep(Duration::from_secs(1)) => {}
            }

            if !self.accepting.load(Ordering::SeqCst) {
                break;
            }

            self.in_flight.fetch_add(1, Ordering::SeqCst);
            sleep(Duration::from_millis(60)).await;
            let previous = self.in_flight.fetch_sub(1, Ordering::SeqCst);
            if previous <= 1 {
                self.drained_notify.notify_waiters();
            }
        }

        tracing::info!(
            component = "http_transport",
            operation = "stop",
            status = "success",
            "http transport loop exited"
        );
    }

    pub async fn shutdown(&self, timeout_duration: Duration) -> bool {
        self.stop_accepting();
        tracing::info!(
            component = "http_transport",
            operation = "shutdown",
            status = "started",
            timeout_ms = timeout_duration.as_millis() as u64,
            "http graceful shutdown started"
        );

        let wait_in_flight = async {
            loop {
                if self.in_flight.load(Ordering::SeqCst) == 0 {
                    break;
                }
                self.drained_notify.notified().await;
            }
        };

        match timeout(timeout_duration, wait_in_flight).await {
            Ok(_) => {
                tracing::info!(
                    component = "http_transport",
                    operation = "shutdown",
                    status = "success",
                    "http graceful shutdown completed"
                );
                true
            }
            Err(_) => {
                tracing::warn!(
                    component = "http_transport",
                    operation = "shutdown",
                    status = "timeout",
                    in_flight = self.in_flight.load(Ordering::SeqCst),
                    "http graceful shutdown timed out"
                );
                false
            }
        }
    }
}
