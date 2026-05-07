use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::Request;

use crate::config::AppConfig;
use crate::model::host::{AgentIdentityState, HostFacts, HostRegistration};
use crate::repository::vm::IdentityStore;
use crate::service::host::collect_host_facts;
use crate::transport::grpc::pb::agent_registry_v1::agent_registry_client::AgentRegistryClient;
use crate::transport::grpc::pb::hypervisor_runtime_v1::runtime_assignment_service_client::RuntimeAssignmentServiceClient;
use crate::transport::grpc::pb::hypervisor_runtime_v1::ResolveRuntimeAssignmentRequest;
use crate::transport::grpc::pb::hypervisor_telemetry_v1::hypervisor_telemetry_service_client::HypervisorTelemetryServiceClient;

use crate::agent::bootstrap::{
    build_channel_for_target, ensure_identity, is_auth_failure, is_fatal_bootstrap_error,
};
use crate::agent::command_ledger::CommandLedger;
use crate::agent::frames::register_frame;
use crate::agent::heartbeat;
use crate::agent::registration::handle_server_message;
use crate::agent::telemetry::run_telemetry_loop;

#[derive(Clone, Debug)]
struct AssignmentCache {
    preferred_dp_id: String,
    preferred_target: String,
    candidates: Vec<String>,
    assignment_epoch: i64,
    refresh_after: Duration,
    expires_at: SystemTime,
}

pub async fn connect_hypervisor(config: AppConfig, shutdown: CancellationToken) -> Result<()> {
    let store = IdentityStore::new(&config.agent);
    let command_ledger = CommandLedger::open(&config.agent.command_ledger_path)?;
    let facts = collect_host_facts(&config);
    let mut assignment_cache: Option<AssignmentCache> = None;
    let mut backoff = config.agent.failover_base_backoff;

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        let identity = ensure_identity(&config, &store, &facts).await?;
        let assignment =
            match resolve_assignment(&config, &identity, &facts, assignment_cache.as_ref()).await {
                Ok(value) => value,
                Err(err) => {
                    if let Some(cache) = assignment_cache.as_ref() {
                        if SystemTime::now() < cache.expires_at && !cache.candidates.is_empty() {
                            cache.clone()
                        } else {
                            tracing::warn!(
                                component = "agent",
                                operation = "resolve_runtime_assignment",
                                status = "retrying",
                                error_message = %err,
                                error_detail = %format!("{err:#}"),
                                error_debug = ?err,
                                retry_in_ms = backoff.as_millis() as u64,
                                "runtime assignment unavailable; will retry"
                            );
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(config.agent.failover_max_backoff);
                            continue;
                        }
                    } else {
                        tracing::warn!(
                            component = "agent",
                            operation = "resolve_runtime_assignment",
                            status = "retrying",
                            error_message = %err,
                            retry_in_ms = backoff.as_millis() as u64,
                            "runtime assignment unavailable; will retry"
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(config.agent.failover_max_backoff);
                        continue;
                    }
                }
            };
        assignment_cache = Some(assignment.clone());
        let mut selector = RuntimeTargetSelector::new(
            assignment.candidates.clone(),
            config.agent.failover_base_backoff,
            config.agent.failover_max_backoff,
        );

        while let Some(target) = selector.current() {
            match run_session(
                &config,
                &store,
                &facts,
                &identity,
                &assignment,
                target,
                &command_ledger,
                shutdown.clone(),
            )
            .await
            {
                Ok(_) => {
                    selector.reset_backoff();
                    backoff = config.agent.failover_base_backoff;
                    break;
                }
                Err(err) => {
                    if is_fatal_bootstrap_error(&err) {
                        return Err(err);
                    }
                    let is_auth = is_auth_failure(&err);
                    let retry_in = selector.failover_delay().max(backoff);
                    tracing::error!(
                        component = "agent",
                        operation = "session",
                        status = "error",
                        target = %target,
                        error_message = %err,
                        error_detail = %format!("{err:#}"),
                        error_debug = ?err,
                        retry_in = ?retry_in,
                        is_auth_failure = is_auth,
                        "hypervisor session failed, rotating runtime target"
                    );
                    if is_auth {
                        store.clear_identity();
                    }
                    selector.advance();
                    if selector.current().is_none() {
                        assignment_cache = None;
                        backoff = (backoff * 2).min(config.agent.failover_max_backoff);
                        break;
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(retry_in) => {},
                        _ = shutdown.cancelled() => return Ok(()),
                    }
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
    identity: &AgentIdentityState,
    assignment: &AssignmentCache,
    runtime_target: &str,
    command_ledger: &CommandLedger,
    shutdown: CancellationToken,
) -> Result<()> {
    let _ = store;
    let channel = build_channel_for_target(config, runtime_target, Some(identity)).await?;
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

    let telemetry_channel =
        build_channel_for_target(config, runtime_target, Some(identity)).await?;
    let telemetry_client = HypervisorTelemetryServiceClient::new(telemetry_channel);
    let telemetry_shutdown = shutdown.clone();
    let telemetry_facts = facts.clone();
    let telemetry_zone = config.app.zone_id.clone();
    let telemetry_interval = config.agent.telemetry_interval.max(Duration::from_secs(5));
    let telemetry_handle = tokio::spawn(async move {
        if telemetry_zone.trim().is_empty() {
            tracing::info!(
                component = "agent",
                operation = "telemetry",
                status = "skipped",
                "zone_id is not assigned yet; telemetry loop will stay idle until zone-aware config is provided"
            );
            telemetry_shutdown.cancelled().await;
            return;
        }
        run_telemetry_loop(
            telemetry_client,
            telemetry_facts,
            telemetry_zone,
            telemetry_shutdown,
            telemetry_interval,
        )
        .await;
    });

    let refresh_shutdown = shutdown.clone();
    let refresh_config = config.clone();
    let refresh_facts = facts.clone();
    let refresh_identity = identity.clone();
    let refresh_assignment = assignment.clone();
    let refresh_target = runtime_target.to_string();
    let (assignment_tx, mut assignment_rx) = mpsc::channel::<()>(1);
    let refresh_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = refresh_shutdown.cancelled() => return,
                _ = tokio::time::sleep(refresh_assignment.refresh_after) => {
                    if let Ok(updated) = resolve_assignment(&refresh_config, &refresh_identity, &refresh_facts, Some(&refresh_assignment)).await {
                        if updated.preferred_target != refresh_target {
                            let _ = assignment_tx.send(()).await;
                            return;
                        }
                    }
                }
            }
        }
    });

    tracing::info!(
        component = "agent",
        operation = "register_host",
        status = "started",
        target = %runtime_target,
        preferred_dp_id = %assignment.preferred_dp_id,
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
            changed = assignment_rx.recv() => {
                if changed.is_some() {
                    heartbeat_handle.abort();
                    telemetry_handle.abort();
                    refresh_handle.abort();
                    return Err(anyhow!("runtime assignment changed"));
                }
            }
            item = response.message() => {
                match item {
                    Ok(Some(frame)) => {
                        if let Err(err) = handle_server_message(frame, &tx, facts, &stream_id, &seq, command_ledger).await {
                            heartbeat_handle.abort();
                            telemetry_handle.abort();
                            refresh_handle.abort();
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
                        refresh_handle.abort();
                        return Err(anyhow!(status));
                    }
                }
            }
        }
    }

    heartbeat_handle.abort();
    telemetry_handle.abort();
    refresh_handle.abort();
    Ok(())
}

async fn resolve_assignment(
    config: &AppConfig,
    identity: &AgentIdentityState,
    facts: &HostFacts,
    current: Option<&AssignmentCache>,
) -> Result<AssignmentCache> {
    let channel =
        build_channel_for_target(config, &config.agent.bootstrap_target_addr, Some(identity))
            .await?;
    let mut client = RuntimeAssignmentServiceClient::new(channel);
    let response = client
        .resolve_runtime_assignment(Request::new(ResolveRuntimeAssignmentRequest {
            agent_id: facts.agent_id.clone(),
            host_id: facts.host_id.clone(),
            zone_id: config.app.zone_id.clone(),
            last_assignment_epoch: current
                .map(|item| item.assignment_epoch)
                .unwrap_or_default(),
            current_dp_id: current
                .map(|item| item.preferred_dp_id.clone())
                .unwrap_or_default(),
        }))
        .await?
        .into_inner();
    let mut candidates = Vec::new();
    for item in response.candidate_dps {
        if item.grpc_addr.trim().is_empty() {
            continue;
        }
        candidates.push(item.grpc_addr.trim().to_string());
    }
    if candidates.is_empty() && !response.preferred_dp_addr.trim().is_empty() {
        candidates.push(response.preferred_dp_addr.trim().to_string());
    }
    if candidates.is_empty() {
        return Err(anyhow!(
            "runtime assignment returned no dataplane candidates"
        ));
    }
    Ok(AssignmentCache {
        preferred_dp_id: response.preferred_dp_id,
        preferred_target: response.preferred_dp_addr.clone(),
        candidates,
        assignment_epoch: response.assignment_epoch,
        refresh_after: Duration::from_secs(response.refresh_after_sec.max(5) as u64),
        expires_at: SystemTime::now()
            + Duration::from_secs((response.refresh_after_sec.max(5) * 2) as u64),
    })
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
            self.index += 1;
            if self.index >= self.targets.len() {
                self.targets.clear();
                return;
            }
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
