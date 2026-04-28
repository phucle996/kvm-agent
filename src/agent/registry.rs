use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, MissedTickBehavior};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tonic::{Code, Request, Status};

use crate::agent::heartbeat;
use crate::config::AppConfig;
use crate::model::host::{AgentIdentityState, HostFacts, HostRegistration};
use crate::repository::vm::IdentityStore;
use crate::service::host::{collect_host_facts, host_registration_from_facts};
use crate::transport::grpc::pb::agent_registry_v1::{
    agent_registry_client::AgentRegistryClient, agent_to_hypervisor, hypervisor_to_agent,
    AgentCommandResult, AgentToHypervisor, BootstrapEnrollAgentRequest, HostInventory,
    HypervisorToAgent, NetworkInterfaceInventory, NodeMetricSample, RegisterHost,
    StoragePoolInventory,
};

const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(5);

pub async fn connect_hypervisor(config: AppConfig, shutdown: CancellationToken) -> Result<()> {
    if !config.agent.enabled {
        tracing::info!(
            component = "agent",
            operation = "connect_hypervisor",
            status = "skipped",
            "agent hypervisor enrollment disabled"
        );
        return Ok(());
    }

    let store = IdentityStore::new(&config.agent);
    let facts = collect_host_facts(&config);

    tracing::info!(
        component = "agent",
        operation = "connect_hypervisor",
        status = "started",
        agent_id = %facts.agent_id,
        host_id = %facts.host_id,
        hypervisor_target = %config.agent.target_addr,
        "hypervisor enrollment loop started"
    );

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        let identity = match ensure_identity(&config, &store, &facts).await {
            Ok(identity) => identity,
            Err(err) => {
                if is_fatal_bootstrap_error(&err) {
                    tracing::error!(
                        component = "agent",
                        operation = "bootstrap_enroll",
                        status = "fatal",
                        error_message = %err,
                        "bootstrap enrollment failed fatally"
                    );
                    return Err(err);
                }

                tracing::warn!(
                    component = "agent",
                    operation = "bootstrap_enroll",
                    status = "retry",
                    error_message = %err,
                    "bootstrap enrollment failed, will retry"
                );
                sleep(DEFAULT_RETRY_DELAY).await;
                continue;
            }
        };

        match register_host(&config, &facts, &identity, shutdown.clone()).await {
            Ok(_) => {
                if shutdown.is_cancelled() {
                    break;
                }
                tracing::warn!(
                    component = "agent",
                    operation = "register_host",
                    status = "closed",
                    "hypervisor session closed, reconnecting"
                );
                sleep(DEFAULT_RETRY_DELAY).await;
            }
            Err(err) => {
                if shutdown.is_cancelled() {
                    break;
                }

                if is_auth_failure(&err) {
                    store.clear_enrollment();
                }

                tracing::warn!(
                    component = "agent",
                    operation = "register_host",
                    status = "retry",
                    error_message = %err,
                    auth_failure = is_auth_failure(&err),
                    "hypervisor session ended, reconnecting"
                );
                sleep(DEFAULT_RETRY_DELAY).await;
            }
        }
    }

    tracing::info!(
        component = "agent",
        operation = "connect_hypervisor",
        status = "stopped",
        "hypervisor enrollment loop stopped"
    );

    Ok(())
}

pub async fn register_host(
    config: &AppConfig,
    facts: &HostFacts,
    identity: &AgentIdentityState,
    shutdown: CancellationToken,
) -> Result<()> {
    let channel = build_channel(config, Some(identity)).await?;
    let mut client = AgentRegistryClient::new(channel);

    let stream_id = new_stream_id(&facts.agent_id);
    let seq = Arc::new(AtomicU64::new(1));
    let (tx, rx) = mpsc::channel::<AgentToHypervisor>(32);
    tx.send(register_frame(facts, &stream_id, next_seq(&seq)))
        .await?;

    let mut response = client
        .agent_control_stream(ReceiverStream::new(rx))
        .await
        .context("open hypervisor agent stream")?
        .into_inner();

    let heartbeat_shutdown = shutdown.clone();
    let heartbeat_tx = tx.clone();
    let heartbeat_facts: HostRegistration = host_registration_from_facts(facts);
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

async fn ensure_identity(
    config: &AppConfig,
    store: &IdentityStore,
    facts: &HostFacts,
) -> Result<AgentIdentityState> {
    if store.has_usable_client_certificate() {
        return store.load_identity();
    }

    bootstrap_enroll(config, store, facts).await
}

async fn bootstrap_enroll(
    config: &AppConfig,
    store: &IdentityStore,
    facts: &HostFacts,
) -> Result<AgentIdentityState> {
    if config.agent.bootstrap_token.trim().is_empty() {
        return Err(anyhow!(
            "bootstrap token is required until the first enrollment succeeds"
        ));
    }

    let private_key = store.ensure_private_key()?;
    let csr_pem = store
        .generate_csr(&private_key, &format!("vm-agent:{}", facts.agent_id))
        .context("generate bootstrap csr")?;

    let channel = build_channel(config, None).await?;
    let mut client = AgentRegistryClient::new(channel);

    let response = client
        .bootstrap_enroll_agent(Request::new(BootstrapEnrollAgentRequest {
            bootstrap_token: config.agent.bootstrap_token.clone(),
            requested_agent_id: facts.agent_id.clone(),
            csr_pem,
            hostname: facts.hostname.clone(),
        }))
        .await
        .context("bootstrap enroll agent")?
        .into_inner();

    if response.agent_id.trim().is_empty() {
        return Err(anyhow!("bootstrap enrollment returned empty agent id"));
    }

    store
        .save_identity(
            response.client_cert_pem.as_bytes(),
            response.ca_cert_pem.as_bytes(),
        )
        .context("persist bootstrap enrollment")?;

    let state = store.load_identity().context("reload enrolled identity")?;
    tracing::info!(
        component = "agent",
        operation = "bootstrap_enroll",
        status = "success",
        agent_id = %response.agent_id,
        cert_not_after = %state.cert_not_after.clone().unwrap_or_else(|| "unknown".to_string()),
        "bootstrap enrollment completed"
    );

    Ok(state)
}

fn register_frame(facts: &HostFacts, stream_id: &str, seq: u64) -> AgentToHypervisor {
    AgentToHypervisor {
        stream_id: stream_id.to_string(),
        seq,
        message: Some(agent_to_hypervisor::Message::RegisterHost(RegisterHost {
            agent_id: facts.agent_id.clone(),
            host_id: facts.host_id.clone(),
            hostname: facts.hostname.clone(),
            private_ip: facts.private_ip.clone(),
            hypervisor_type: facts.hypervisor_type.clone(),
            agent_version: facts.agent_version.clone(),
            capabilities_json: facts.capabilities_json.clone(),
            cpu_cores: facts.cpu_cores,
            memory_bytes: facts.memory_bytes,
            disk_bytes: facts.disk_bytes,
        })),
    }
}

async fn handle_server_message(
    frame: HypervisorToAgent,
    tx: &mpsc::Sender<AgentToHypervisor>,
    facts: &HostFacts,
    stream_id: &str,
    seq: &Arc<AtomicU64>,
) -> Result<()> {
    match frame.message {
        Some(hypervisor_to_agent::Message::RegisterAck(ack)) => {
            tracing::info!(
                component = "agent",
                operation = "register_host",
                status = "success",
                host_id = %ack.host_id,
                ack_status = %ack.status,
                "host registration acknowledged"
            );
        }
        Some(hypervisor_to_agent::Message::HeartbeatAck(ack)) => {
            tracing::debug!(
                component = "agent",
                operation = "heartbeat_ack",
                status = "success",
                host_id = %ack.host_id,
                ack_status = %ack.status,
                "heartbeat acknowledged"
            );
        }
        Some(hypervisor_to_agent::Message::Command(command)) => {
            tracing::info!(
                component = "agent",
                operation = "command",
                status = "received",
                command_id = %command.command_id,
                command_type = %command.r#type,
                "received hypervisor command"
            );
            let result = execute_agent_command(&command.r#type, &command.payload_json).await;
            let (status, result_json, error_message) = match result {
                Ok(value) => ("succeeded".to_string(), value, String::new()),
                Err(err) => ("failed".to_string(), "{}".to_string(), err.to_string()),
            };
            tx.send(AgentToHypervisor {
                stream_id: stream_id.to_string(),
                seq: next_seq(seq),
                message: Some(agent_to_hypervisor::Message::CommandResult(
                    AgentCommandResult {
                        agent_id: facts.agent_id.clone(),
                        host_id: facts.host_id.clone(),
                        command_id: command.command_id,
                        status,
                        result_json,
                        error_message,
                        completed_at: Some(system_time_to_timestamp(SystemTime::now())),
                    },
                )),
            })
            .await
            .context("send command result")?;
        }
        None => {}
    }
    Ok(())
}

async fn build_channel(
    config: &AppConfig,
    identity: Option<&AgentIdentityState>,
) -> Result<Channel> {
    let target = normalize_endpoint(&config.agent.target_addr);
    let use_tls = target.starts_with("https://");

    // During bootstrap, if we don't have a CA on disk, fallback to insecure TLS
    if identity.is_none() && use_tls && std::fs::metadata(&config.agent.ca_path).is_err() {
        tracing::warn!(
            component = "agent",
            operation = "build_channel",
            "no CA certificate found for bootstrap, using insecure TLS (skip verification)"
        );

        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        
        config.alpn_protocols = vec![b"h2".to_vec()];
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(NoCertificateVerification));

        let target_insecure = target.replace("https://", "http://");
        return Ok(Endpoint::from_shared(target_insecure)?
            .connect_with_connector(tower::service_fn(move |uri: http::Uri| {
                let host = uri.host().unwrap_or("localhost").to_string();
                let port = uri.port_u16().unwrap_or(443);
                let addr = format!("{}:{}", host, port);
                let config = config.clone();

                async move {
                    tracing::info!(%addr, "connecting to hypervisor");
                    let stream = tokio::net::TcpStream::connect(&addr).await
                        .map_err(|e| { tracing::error!(%e, "tcp connect failed"); e })?;
                    
                    tracing::info!("performing tls handshake");
                    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
                    let domain = rustls::pki_types::ServerName::try_from(host.clone())
                        .map_err(|e| { tracing::error!(%e, "invalid server name"); std::io::Error::new(std::io::ErrorKind::InvalidInput, e) })?
                        .to_owned();
                    
                    let tls_stream = connector.connect(domain, stream).await
                        .map_err(|e| { tracing::error!(%e, "tls handshake failed"); e })?;
                    
                    tracing::info!("tls handshake successful, wrapping in tokioio");
                    Ok::<_, std::io::Error>(hyper_util::rt::tokio::TokioIo::new(tls_stream))
                }
            }))
            .await?);
    }

    let endpoint = Endpoint::from_shared(target).context("build hypervisor endpoint")?;

    if use_tls {
        let mut tls = ClientTlsConfig::new();
        let server_name = server_name(config);
        tls = tls.domain_name(server_name);

        let ca_pem = match identity {
            Some(state) if !state.ca_bundle_pem.is_empty() => state.ca_bundle_pem.clone(),
            _ => std::fs::read(&config.agent.ca_path)
                .with_context(|| format!("read ca bundle {}", config.agent.ca_path))?,
        };
        tls = tls.ca_certificate(Certificate::from_pem(ca_pem));

        if let Some(state) = identity {
            if !state.client_cert_pem.is_empty() && !state.client_key_pem.is_empty() {
                tls = tls.identity(Identity::from_pem(
                    state.client_cert_pem.clone(),
                    state.client_key_pem.clone(),
                ));
            }
        }

        Ok(endpoint.tls_config(tls)?.connect().await?)
    } else {
        Ok(endpoint.connect().await?)
    }
}

async fn run_telemetry_loop(
    tx: mpsc::Sender<AgentToHypervisor>,
    facts: HostFacts,
    stream_id: String,
    seq: Arc<AtomicU64>,
    shutdown: CancellationToken,
    interval_duration: Duration,
) {
    let mut ticker = interval(interval_duration.max(Duration::from_secs(5)));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    if tx
        .send(host_inventory_frame(&facts, &stream_id, next_seq(&seq)))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                if tx.send(node_metric_frame(&facts, &stream_id, next_seq(&seq))).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn host_inventory_frame(facts: &HostFacts, stream_id: &str, seq: u64) -> AgentToHypervisor {
    AgentToHypervisor {
        stream_id: stream_id.to_string(),
        seq,
        message: Some(agent_to_hypervisor::Message::HostInventory(HostInventory {
            agent_id: facts.agent_id.clone(),
            host_id: facts.host_id.clone(),
            storage_pools: vec![StoragePoolInventory {
                name: "default".to_string(),
                driver: "dir".to_string(),
                path: "/var/lib/libvirt/images".to_string(),
                total_bytes: facts.disk_bytes,
                used_bytes: 0,
                status: "active".to_string(),
                metadata_json: "{}".to_string(),
            }],
            network_interfaces: vec![NetworkInterfaceInventory {
                name: "default".to_string(),
                mac_address: String::new(),
                ipv4_address: facts.private_ip.clone(),
                ipv6_address: String::new(),
                speed_mbps: 0,
                status: "unknown".to_string(),
                metadata_json: "{}".to_string(),
            }],
            collected_at: Some(system_time_to_timestamp(SystemTime::now())),
        })),
    }
}

fn node_metric_frame(facts: &HostFacts, stream_id: &str, seq: u64) -> AgentToHypervisor {
    let (load1, load5, load15) = load_average();
    let (ram_used_gib, ssd_used_gib) = usage_snapshot_gib();
    AgentToHypervisor {
        stream_id: stream_id.to_string(),
        seq,
        message: Some(agent_to_hypervisor::Message::NodeMetric(NodeMetricSample {
            agent_id: facts.agent_id.clone(),
            host_id: facts.host_id.clone(),
            cpu_used_percent: 0.0,
            ram_used_gib,
            ssd_used_gib,
            network_rx_bps: 0,
            network_tx_bps: 0,
            load_avg_1m: load1,
            load_avg_5m: load5,
            load_avg_15m: load15,
            sampled_at: Some(system_time_to_timestamp(SystemTime::now())),
        })),
    }
}

async fn execute_agent_command(command_type: &str, payload_json: &str) -> Result<String> {
    let payload = if payload_json.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(payload_json).unwrap_or_else(|_| json!({ "raw": payload_json }))
    };
    Ok(json!({
        "command_type": command_type,
        "payload": payload,
        "runtime_driver": "kvm",
        "accepted": true
    })
    .to_string())
}

fn new_stream_id(agent_id: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();
    format!("{agent_id}-{now}")
}

fn next_seq(seq: &Arc<AtomicU64>) -> u64 {
    seq.fetch_add(1, Ordering::SeqCst)
}

fn load_average() -> (f64, f64, f64) {
    let Ok(contents) = std::fs::read_to_string("/proc/loadavg") else {
        return (0.0, 0.0, 0.0);
    };
    let mut parts = contents.split_whitespace();
    let one = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let five = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let fifteen = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    (one, five, fifteen)
}

fn usage_snapshot_gib() -> (f64, f64) {
    let ram_used = 0.0;
    let ssd_used = 0.0;
    (ram_used, ssd_used)
}

fn normalize_endpoint(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

fn server_name(config: &AppConfig) -> String {
    let name = config.agent.server_name.trim();
    if !name.is_empty() {
        return name.to_string();
    }

    let raw = config.agent.target_addr.trim();
    let without_scheme = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .unwrap_or(raw);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    host_port
        .split_once(':')
        .map(|(host, _)| host.to_string())
        .unwrap_or_else(|| host_port.to_string())
}

fn is_fatal_bootstrap_error(err: &anyhow::Error) -> bool {
    grpc_status_in_chain(err)
        .map(|status| matches!(status.code(), Code::Unauthenticated | Code::InvalidArgument))
        .unwrap_or(false)
}

fn is_auth_failure(err: &anyhow::Error) -> bool {
    grpc_status_in_chain(err)
        .map(|status| {
            matches!(
                status.code(),
                Code::Unauthenticated | Code::PermissionDenied
            )
        })
        .unwrap_or(false)
}

fn grpc_status_in_chain(err: &anyhow::Error) -> Option<&Status> {
    for cause in err.chain() {
        if let Some(status) = cause.downcast_ref::<Status>() {
            return Some(status);
        }
    }
    None
}

fn system_time_to_timestamp(time: SystemTime) -> prost_types::Timestamp {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    prost_types::Timestamp {
        seconds: duration.as_secs() as i64,
        nanos: duration.subsec_nanos() as i32,
    }
}

#[derive(Debug)]
struct NoCertificateVerification;

impl rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
