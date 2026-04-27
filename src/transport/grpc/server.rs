use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Mutex, Notify};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic_health::server::health_reporter;
use tonic_health::ServingStatus;

#[derive(Clone, Debug)]
pub struct GrpcServer {
    bind_addr: String,
    accepting: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    drained_notify: Arc<Notify>,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl GrpcServer {
    pub fn new(bind_addr: String) -> Self {
        Self {
            bind_addr,
            accepting: Arc::new(AtomicBool::new(true)),
            running: Arc::new(AtomicBool::new(false)),
            drained_notify: Arc::new(Notify::new()),
            shutdown_tx: Arc::new(Mutex::new(None)),
        }
    }

    pub fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        let shutdown_tx = self.shutdown_tx.clone();
        tokio::spawn(async move {
            if let Some(tx) = shutdown_tx.lock().await.take() {
                let _ = tx.send(());
            }
        });

        tracing::info!(
            component = "grpc_transport",
            operation = "stop_accepting",
            status = "success",
            bind_addr = %self.bind_addr,
            "grpc server stopped accepting requests"
        );
    }

    pub async fn run(&self, shutdown: CancellationToken) {
        let addr: SocketAddr = match self.bind_addr.parse() {
            Ok(addr) => addr,
            Err(err) => {
                tracing::error!(
                    component = "grpc_transport",
                    operation = "start",
                    status = "error",
                    bind_addr = %self.bind_addr,
                    error_message = %err,
                    "invalid grpc bind address"
                );
                self.drained_notify.notify_waiters();
                return;
            }
        };

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        {
            let mut guard = self.shutdown_tx.lock().await;
            *guard = Some(shutdown_tx);
        }

        let (mut reporter, health_service) = health_reporter();
        reporter
            .set_service_status("", ServingStatus::Serving)
            .await;

        self.running.store(true, Ordering::SeqCst);
        tracing::info!(
            component = "grpc_transport",
            operation = "start",
            status = "started",
            bind_addr = %addr,
            "grpc transport started"
        );

        let result = Server::builder()
            .add_service(health_service)
            .serve_with_shutdown(addr, async move {
                tokio::select! {
                    _ = shutdown.cancelled() => {}
                    _ = shutdown_rx => {}
                }
            })
            .await;

        self.running.store(false, Ordering::SeqCst);
        self.drained_notify.notify_waiters();

        match result {
            Ok(_) => {
                tracing::info!(
                    component = "grpc_transport",
                    operation = "stop",
                    status = "success",
                    bind_addr = %addr,
                    "grpc transport loop exited"
                );
            }
            Err(err) => {
                tracing::error!(
                    component = "grpc_transport",
                    operation = "stop",
                    status = "error",
                    bind_addr = %addr,
                    error_message = %err,
                    "grpc transport exited with error"
                );
            }
        }
    }

    pub async fn shutdown(&self, timeout_duration: Duration) -> bool {
        self.stop_accepting();
        tracing::info!(
            component = "grpc_transport",
            operation = "shutdown",
            status = "started",
            timeout_ms = timeout_duration.as_millis() as u64,
            "grpc graceful shutdown started"
        );

        let wait_running = async {
            loop {
                if !self.running.load(Ordering::SeqCst) {
                    break;
                }
                self.drained_notify.notified().await;
            }
        };

        match timeout(timeout_duration, wait_running).await {
            Ok(_) => {
                tracing::info!(
                    component = "grpc_transport",
                    operation = "shutdown",
                    status = "success",
                    "grpc graceful shutdown completed"
                );
                true
            }
            Err(_) => {
                tracing::warn!(
                    component = "grpc_transport",
                    operation = "shutdown",
                    status = "timeout",
                    "grpc graceful shutdown timed out"
                );
                false
            }
        }
    }
}
