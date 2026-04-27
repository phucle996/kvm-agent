use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::app::state::AppStateStore;
use crate::config::AppConfig;
use crate::transport::grpc::server::GrpcServer;
use crate::worker::dispatcher::WorkerDispatcher;
use crate::worker::pool::WorkerPool;

#[derive(Clone)]
pub struct AppContext {
    pub config: AppConfig,
    pub state: AppStateStore,
    pub shutdown_token: CancellationToken,
    pub tasks: TaskTracker,
    pub worker_pool: WorkerPool,
    pub dispatcher: WorkerDispatcher,
    pub grpc_server: GrpcServer,
    pub shutdown_timeout: Duration,
}

impl AppContext {
    pub fn new(config: AppConfig) -> Self {
        let shutdown_timeout = config.app.shutdown_timeout;
        Self {
            config: config.clone(),
            state: AppStateStore::new(),
            shutdown_token: CancellationToken::new(),
            tasks: TaskTracker::new(),
            worker_pool: WorkerPool::new(config.worker.max_workers),
            dispatcher: WorkerDispatcher::new(),
            grpc_server: GrpcServer::new(config.grpc.bind_addr.clone()),
            shutdown_timeout,
        }
    }
}
