use anyhow::{anyhow, Context, Result};
use std::sync::Arc;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tonic::{Code, Request, Status};

use crate::config::AppConfig;
use crate::model::host::{AgentIdentityState, HostFacts};
use crate::repository::vm::IdentityStore;
use crate::transport::grpc::pb::agent_registry_v1::agent_registry_client::AgentRegistryClient;
use crate::transport::grpc::pb::agent_registry_v1::BootstrapEnrollAgentRequest;

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

    let channel =
        build_channel_for_target(config, &config.agent.bootstrap_target_addr, None).await?;
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

pub async fn build_channel(
    config: &AppConfig,
    identity: Option<&AgentIdentityState>,
) -> Result<Channel> {
    build_channel_for_target(config, &config.agent.bootstrap_target_addr, identity).await
}

pub async fn build_channel_for_target(
    config: &AppConfig,
    target_addr: &str,
    identity: Option<&AgentIdentityState>,
) -> Result<Channel> {
    let target = normalize_endpoint(target_addr);
    let use_tls = target.starts_with("https://");

    if identity.is_none() && use_tls && std::fs::metadata(&config.agent.ca_path).is_err() {
        let connect_timeout = config.agent.connect_timeout;
        tracing::warn!(
            component = "agent",
            operation = "build_channel",
            target = %target_addr,
            "no CA certificate found for bootstrap, using insecure TLS (skip verification)"
        );

        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let mut rustls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        rustls_config.alpn_protocols = vec![b"h2".to_vec()];
        rustls_config
            .dangerous()
            .set_certificate_verifier(Arc::new(NoCertificateVerification));

        let target_insecure = target.replace("https://", "http://");
        return Ok(Endpoint::from_shared(target_insecure)?
            .connect_timeout(connect_timeout)
            .connect_with_connector(tower::service_fn(move |uri: http::Uri| {
                let host = uri.host().unwrap_or("localhost").to_string();
                let port = uri.port_u16().unwrap_or(443);
                let addr = format!("{}:{}", host, port);
                let rustls_config = rustls_config.clone();

                async move {
                    let stream = tokio::time::timeout(
                        connect_timeout,
                        tokio::net::TcpStream::connect(&addr),
                    )
                    .await
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::TimedOut, e))??;
                    let connector = tokio_rustls::TlsConnector::from(Arc::new(rustls_config));
                    let domain = rustls::pki_types::ServerName::try_from(host.clone())
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?
                        .to_owned();

                    let tls_stream = connector.connect(domain, stream).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::tokio::TokioIo::new(tls_stream))
                }
            }))
            .await?);
    }

    let endpoint = Endpoint::from_shared(target)
        .context("build hypervisor endpoint")?
        .connect_timeout(config.agent.connect_timeout);

    if use_tls {
        let mut tls = ClientTlsConfig::new();
        let server_name = server_name_for_target(config, target_addr);
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

fn normalize_endpoint(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

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

pub fn is_fatal_bootstrap_error(err: &anyhow::Error) -> bool {
    grpc_status_in_chain(err)
        .map(|status| matches!(status.code(), Code::Unauthenticated | Code::InvalidArgument))
        .unwrap_or(false)
}

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

fn grpc_status_in_chain(err: &anyhow::Error) -> Option<&Status> {
    for cause in err.chain() {
        if let Some(status) = cause.downcast_ref::<Status>() {
            return Some(status);
        }
    }
    None
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
