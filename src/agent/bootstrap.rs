use anyhow::{anyhow, Context, Result};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tonic::{Code, Request, Status};

use crate::config::AppConfig;
use crate::model::host::{AgentIdentityState, HostFacts};
use crate::repository::vm::IdentityStore;
use crate::transport::grpc::pb::agent_registry_v1::agent_registry_client::AgentRegistryClient;
use crate::transport::grpc::pb::agent_registry_v1::BootstrapEnrollAgentRequest;

// ensure_identity reuses an existing enrolled client identity when present,
// otherwise it starts the one-time bootstrap enrollment flow.
pub async fn ensure_identity(
    config: &AppConfig,
    store: &IdentityStore,
    facts: &HostFacts,
) -> Result<AgentIdentityState> {
    if store.has_usable_client_certificate() {
        return store.load_identity();
    }

    bootstrap_enroll(config, store, facts).await
}

// bootstrap_enroll performs the pre-mTLS enrollment exchange: create CSR,
// call controlplane bootstrap RPC with the one-time token, then persist the
// issued client identity for future runtime mTLS connections.
pub async fn bootstrap_enroll(
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

    // Bootstrap is the only RPC path that runs before the agent has a client
    // certificate. We keep a dedicated channel builder so that pre-enrollment
    // TLS and post-enrollment mTLS stay visibly separate in code.
    let channel = build_bootstrap_channel(config).await?;
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
    if response.runtime_target_addr.trim().is_empty() {
        return Err(anyhow!(
            "bootstrap enrollment returned empty runtime target addr"
        ));
    }

    store
        .save_identity(
            response.client_cert_pem.as_bytes(),
            response.ca_cert_pem.as_bytes(),
            &response.runtime_target_addr,
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

// build_mtls_channel_for_controlplane resolves the best runtime target and
// opens the post-enrollment mTLS channel used for normal agent traffic.
pub async fn build_mtls_channel_for_controlplane(
    config: &AppConfig,
    identity: &AgentIdentityState,
) -> Result<Channel> {
    let target = identity
        .runtime_target_addr
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let configured = config.agent.runtime_target_addr.trim();
            if configured.is_empty() {
                None
            } else {
                Some(configured)
            }
        })
        .unwrap_or(config.agent.bootstrap_target_addr.as_str());
    build_mtls_channel_for_target(config, target, identity).await
}

// build_mtls_channel_for_target always enforces mTLS for runtime traffic by
// requiring CA material plus the enrolled client cert/key pair.
pub async fn build_mtls_channel_for_target(
    config: &AppConfig,
    target_addr: &str,
    identity: &AgentIdentityState,
) -> Result<Channel> {
    // Every RPC after bootstrap must use mTLS. If the target does not already
    // declare a scheme we force it to TLS so the runtime path cannot downgrade.
    let target = normalize_mtls_endpoint(target_addr);
    let endpoint = Endpoint::from_shared(target)
        .context("build hypervisor mTLS endpoint")?
        .connect_timeout(config.agent.connect_timeout);

    if identity.ca_bundle_pem.is_empty() {
        return Err(anyhow!(
            "missing CA bundle for mTLS controlplane/dataplane connection"
        ));
    }
    if identity.client_cert_pem.is_empty() || identity.client_key_pem.is_empty() {
        return Err(anyhow!(
            "missing client certificate or private key for mTLS controlplane/dataplane connection"
        ));
    }

    let mut tls = ClientTlsConfig::new();
    tls = tls.domain_name(server_name_for_target(config, target_addr));
    tls = tls.ca_certificate(Certificate::from_pem(identity.ca_bundle_pem.clone()));
    tls = tls.identity(Identity::from_pem(
        identity.client_cert_pem.clone(),
        identity.client_key_pem.clone(),
    ));

    Ok(endpoint.tls_config(tls)?.connect().await?)
}

// build_bootstrap_channel opens the initial enrollment transport and follows
// the configured bootstrap endpoint scheme: plaintext for http, one-way TLS
// for https before the agent has a client certificate.
async fn build_bootstrap_channel(config: &AppConfig) -> Result<Channel> {
    let target = normalize_bootstrap_endpoint(&config.agent.bootstrap_target_addr)?;
    let endpoint = Endpoint::from_shared(target.clone())
        .context("build hypervisor bootstrap endpoint")?
        .connect_timeout(config.agent.connect_timeout);

    if target.starts_with("http://") {
        return Ok(endpoint.connect().await?);
    }

    let mut tls = ClientTlsConfig::new();
    tls = tls.domain_name(server_name_for_target(
        config,
        &config.agent.bootstrap_target_addr,
    ));
    let ca_pem = std::fs::read(&config.agent.ca_path)
        .with_context(|| format!("read ca bundle {}", config.agent.ca_path))?;
    tls = tls.ca_certificate(Certificate::from_pem(ca_pem));
    Ok(endpoint.tls_config(tls)?.connect().await?)
}

// normalize_bootstrap_endpoint preserves an explicit bootstrap scheme and
// defaults a bare host:port value to plaintext bootstrap.
pub fn normalize_bootstrap_endpoint(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("AGENT_BOOTSTRAP_TARGET_ADDR must not be empty"));
    }
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return Ok(trimmed.to_string());
    }
    Ok(format!("http://{trimmed}"))
}

// normalize_mtls_endpoint prevents runtime downgrade by forcing the runtime
// controlplane/dataplane target to use https.
fn normalize_mtls_endpoint(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("https://") {
        return trimmed.to_string();
    }
    if trimmed.starts_with("http://") {
        return format!("https://{}", trimmed.trim_start_matches("http://"));
    }
    format!("https://{trimmed}")
}

// server_name_for_target picks the TLS SNI/server-name override when set,
// otherwise derives it from the target host portion of the address.
fn server_name_for_target(config: &AppConfig, target_addr: &str) -> String {
    let name = config.agent.server_name.trim();
    if !name.is_empty() {
        return name.to_string();
    }

    let raw = target_addr.trim();
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

// is_fatal_bootstrap_error marks bootstrap failures that should stop retrying
// because the token/request itself is invalid rather than temporarily failing.
pub fn is_fatal_bootstrap_error(err: &anyhow::Error) -> bool {
    grpc_status_in_chain(err)
        .map(|status| matches!(status.code(), Code::Unauthenticated | Code::InvalidArgument))
        .unwrap_or(false)
}

// is_auth_failure identifies gRPC authn/authz failures so callers can report
// credential problems separately from transient transport errors.
pub fn is_auth_failure(err: &anyhow::Error) -> bool {
    grpc_status_in_chain(err)
        .map(|status| {
            matches!(
                status.code(),
                Code::Unauthenticated | Code::PermissionDenied
            )
        })
        .unwrap_or(false)
}

// grpc_status_in_chain walks the anyhow error chain to recover the original
// tonic gRPC status when higher layers wrapped it with extra context.
fn grpc_status_in_chain(err: &anyhow::Error) -> Option<&Status> {
    for cause in err.chain() {
        if let Some(status) = cause.downcast_ref::<Status>() {
            return Some(status);
        }
    }
    None
}
