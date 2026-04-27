use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::model::host::HostRegistration;
use crate::transport::grpc::pb::agent_registry_v1::{
    agent_to_hypervisor, AgentHeartbeat, AgentToHypervisor,
};

pub async fn run_heartbeat_loop(
    tx: mpsc::Sender<AgentToHypervisor>,
    registration: HostRegistration,
    stream_id: String,
    seq: Arc<AtomicU64>,
    shutdown: CancellationToken,
    interval_duration: Duration,
) {
    let interval_duration = interval_duration.max(Duration::from_secs(1));
    let mut ticker = interval(interval_duration);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    tracing::info!(
        component = "agent",
        operation = "heartbeat_loop",
        status = "started",
        agent_id = %registration.agent_id,
        host_id = %registration.host_id,
        interval_secs = interval_duration.as_secs(),
        "heartbeat loop started"
    );

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!(
                    component = "agent",
                    operation = "heartbeat_loop",
                    status = "stopping",
                    "shutdown requested for heartbeat loop"
                );
                break;
            }
            _ = ticker.tick() => {
                let heartbeat = AgentToHypervisor {
                    stream_id: stream_id.clone(),
                    seq: seq.fetch_add(1, Ordering::SeqCst),
                    message: Some(agent_to_hypervisor::Message::Heartbeat(AgentHeartbeat {
                        agent_id: registration.agent_id.clone(),
                        host_id: registration.host_id.clone(),
                        status: "online".to_string(),
                        last_seen_at: Some(system_time_to_timestamp(SystemTime::now())),
                    })),
                };

                if tx.send(heartbeat).await.is_err() {
                    tracing::warn!(
                        component = "agent",
                        operation = "heartbeat_loop",
                        status = "closed",
                        "heartbeat channel closed"
                    );
                    break;
                }

                tracing::debug!(
                    component = "agent",
                    operation = "heartbeat",
                    status = "sent",
                    agent_id = %registration.agent_id,
                    host_id = %registration.host_id,
                    "heartbeat sent"
                );
            }
        }
    }

    tracing::info!(
        component = "agent",
        operation = "heartbeat_loop",
        status = "stopped",
        agent_id = %registration.agent_id,
        host_id = %registration.host_id,
        "heartbeat loop stopped"
    );
}

fn system_time_to_timestamp(time: SystemTime) -> prost_types::Timestamp {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    prost_types::Timestamp {
        seconds: duration.as_secs() as i64,
        nanos: duration.subsec_nanos() as i32,
    }
}
