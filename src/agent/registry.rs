use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::Request;

use crate::config::AppConfig;
use crate::model::host::{HostFacts, HostRegistration};
use crate::repository::vm::IdentityStore;
use crate::service::host::collect_host_facts;
use crate::transport::grpc::pb::agent_registry_v1::agent_registry_client::AgentRegistryClient;

use crate::agent::bootstrap::{
    build_channel_for_target, ensure_identity, is_auth_failure, is_fatal_bootstrap_error,
};
use crate::agent::frames::register_frame;
use crate::agent::heartbeat;
use crate::agent::registration::handle_server_message;
use crate::agent::telemetry::run_telemetry_loop;

pub async fn connect_hypervisor(config: AppConfig, shutdown: CancellationToken) -> Result<()> {
    let store = IdentityStore::new(&config.agent);
    let facts = collect_host_facts(&config);
    let targets = config.agent.runtime_targets();
    let mut selector = RuntimeTargetSelector::new(
        targets,
        config.agent.failover_base_backoff,
        config.agent.failover_max_backoff,
    );

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        let target = selector
            .current()
            .ok_or_else(|| anyhow!("no runtime target configured"))?;
        match run_session(&config, &store, &facts, target, shutdown.clone()).await {
            Ok(_) => {
                selector.reset_backoff();
            }
            Err(err) => {
                if is_fatal_bootstrap_error(&err) {
                    return Err(err);
                }

                let is_auth = is_auth_failure(&err);
                let retry_in = selector.failover_delay();
                tracing::error!(
                    component = "agent",
                    operation = "session",
                    status = "error",
                    target = %target,
                    error_message = %err,
                    retry_in = ?retry_in,
                    is_auth_failure = is_auth,
                    "hypervisor session failed, rotating runtime target"
                );

                if is_auth {
                    store.clear_identity();
                }

                selector.advance();
                tokio::select! {
                    _ = tokio::time::sleep(retry_in) => {},
                    _ = shutdown.cancelled() => break,
                }
            }
        }
    }

    Ok(())
}

async fn run_session(
    config: &AppConfig,
    store: &IdentityStore,
    facts: &HostFacts,
    runtime_target: &str,
    shutdown: CancellationToken,
) -> Result<()> {
    let identity = ensure_identity(config, store, facts).await?;
    let channel = build_channel_for_target(config, runtime_target, Some(&identity)).await?;
    let mut client = AgentRegistryClient::new(channel);

    let (tx, rx) = mpsc::channel(100);
    let stream_id = new_stream_id(&facts.agent_id);
    let seq = Arc::new(AtomicU64::new(1));

    tx.send(register_frame(
        facts,
        &stream_id,
        seq.fetch_add(1, Ordering::SeqCst),
    ))
    .await
    .map_err(|e| anyhow!("failed to send initial register frame: {e}"))?;

    let outbound_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut response = client
        .agent_control_stream(Request::new(outbound_stream))
        .await?
        .into_inner();

    let heartbeat_shutdown = shutdown.clone();
    let heartbeat_tx = tx.clone();
    let heartbeat_facts = host_registration_from_facts(facts);
    let heartbeat_interval = config.agent.heartbeat_interval;
    let heartbeat_stream_id = stream_id.clone();
    let heartbeat_seq = seq.clone();
    let heartbeat_handle = tokio::spawn(async move {
        heartbeat::run_heartbeat_loop(
            heartbeat_tx,
            heartbeat_facts,
            heartbeat_stream_id,
            heartbeat_seq,
            heartbeat_shutdown,
            heartbeat_interval,
        )
        .await;
    });

    let telemetry_shutdown = shutdown.clone();
    let telemetry_tx = tx.clone();
    let telemetry_facts = facts.clone();
    let telemetry_stream_id = stream_id.clone();
    let telemetry_seq = seq.clone();
    let telemetry_interval = config.agent.heartbeat_interval.max(Duration::from_secs(5));
    let telemetry_handle = tokio::spawn(async move {
        run_telemetry_loop(
            telemetry_tx,
            telemetry_facts,
            telemetry_stream_id,
            telemetry_seq,
            telemetry_shutdown,
            telemetry_interval,
        )
        .await;
    });

    tracing::info!(
        component = "agent",
        operation = "register_host",
        status = "started",
        target = %runtime_target,
        agent_id = %facts.agent_id,
        host_id = %facts.host_id,
        stream_id = %stream_id,
        "registering host on hypervisor"
    );

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!(
                    component = "agent",
                    operation = "register_host",
                    status = "stopping",
                    target = %runtime_target,
                    "shutdown requested for host session"
                );
                break;
            }
            item = response.message() => {
                match item {
                    Ok(Some(frame)) => {
                        if let Err(err) = handle_server_message(frame, &tx, facts, &stream_id, &seq).await {
                            heartbeat_handle.abort();
                            telemetry_handle.abort();
                            return Err(err);
                        }
                    }
                    Ok(None) => {
                        tracing::warn!(
                            component = "agent",
                            operation = "register_host",
                            status = "closed",
                            target = %runtime_target,
                            "hypervisor closed the agent stream"
                        );
                        break;
                    }
                    Err(status) => {
                        heartbeat_handle.abort();
                        telemetry_handle.abort();
                        return Err(anyhow!(status));
                    }
                }
            }
        }
    }

    heartbeat_handle.abort();
    telemetry_handle.abort();
    Ok(())
}

fn new_stream_id(agent_id: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();
    format!("{agent_id}-{now}")
}

fn host_registration_from_facts(facts: &HostFacts) -> HostRegistration {
    HostRegistration {
        agent_id: facts.agent_id.clone(),
        host_id: facts.host_id.clone(),
        hostname: facts.hostname.clone(),
        private_ip: facts.private_ip.clone(),
        hypervisor_type: facts.hypervisor_type.clone(),
        agent_version: facts.agent_version.clone(),
        capabilities_json: facts.capabilities_json.clone(),
        cpu_cores: facts.cpu_cores,
        cpu_threads: facts.cpu_threads,
        memory_bytes: facts.memory_bytes,
        disk_bytes: facts.disk_bytes,
        gpu_cores: facts.gpu_cores,
        gpu_memory_bytes: facts.gpu_memory_bytes,
        cpu_model: facts.cpu_model.clone(),
        ram_model: facts.ram_model.clone(),
        disk_model: facts.disk_model.clone(),
        gpu_model: facts.gpu_model.clone(),
        network_interfaces: facts.network_interfaces.clone(),
    }
}

struct RuntimeTargetSelector {
    targets: Vec<String>,
    index: usize,
    backoff: Duration,
    base_backoff: Duration,
    max_backoff: Duration,
}

impl RuntimeTargetSelector {
    fn new(targets: Vec<String>, base_backoff: Duration, max_backoff: Duration) -> Self {
        Self {
            targets,
            index: 0,
            backoff: base_backoff,
            base_backoff,
            max_backoff,
        }
    }

    fn current(&self) -> Option<&str> {
        if self.targets.is_empty() {
            return None;
        }
        self.targets
            .get(self.index % self.targets.len())
            .map(String::as_str)
    }

    fn advance(&mut self) {
        if !self.targets.is_empty() {
            self.index = (self.index + 1) % self.targets.len();
        }
        self.backoff = (self.backoff * 2).min(self.max_backoff);
    }

    fn failover_delay(&self) -> Duration {
        let jitter_cap = (self.backoff.as_millis() / 5).max(1) as u64;
        let jitter = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_millis() as u64)
            % jitter_cap;
        self.backoff + Duration::from_millis(jitter)
    }

    fn reset_backoff(&mut self) {
        self.backoff = self.base_backoff;
    }
}
