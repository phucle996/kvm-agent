use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::worker::pool::WorkerPool;

#[derive(Clone, Debug)]
pub struct WorkerDispatcher {
    polling: Arc<AtomicBool>,
}

impl WorkerDispatcher {
    pub fn new() -> Self {
        Self {
            polling: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn is_polling(&self) -> bool {
        self.polling.load(Ordering::SeqCst)
    }

    pub fn stop_polling(&self) {
        self.polling.store(false, Ordering::SeqCst);
        tracing::info!(
            component = "worker_dispatcher",
            operation = "stop_polling",
            status = "success",
            "worker dispatcher stopped polling"
        );
    }

    pub async fn run(&self, shutdown: CancellationToken, worker_pool: WorkerPool) {
        tracing::info!(
            component = "worker_dispatcher",
            operation = "run",
            status = "started",
            "worker dispatcher loop started"
        );

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    self.stop_polling();
                    break;
                }
                _ = sleep(Duration::from_millis(500)) => {}
            }

            if !self.is_polling() {
                break;
            }

            if !worker_pool.try_start_job() {
                continue;
            }

            tracing::debug!(
                component = "worker_dispatcher",
                operation = "dispatch_job",
                status = "started",
                in_flight = worker_pool.in_flight(),
                "dispatched worker job"
            );

            sleep(Duration::from_millis(150)).await;

            worker_pool.finish_job();
            tracing::debug!(
                component = "worker_dispatcher",
                operation = "dispatch_job",
                status = "success",
                in_flight = worker_pool.in_flight(),
                "worker job completed"
            );
        }

        tracing::info!(
            component = "worker_dispatcher",
            operation = "run",
            status = "stopped",
            "worker dispatcher loop stopped"
        );
    }
}
