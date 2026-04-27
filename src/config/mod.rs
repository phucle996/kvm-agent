pub mod agent;
pub mod app;
pub mod grpc;
pub mod runtime;
pub mod worker;

use std::env;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use crate::config::agent::AgentConfig;
use crate::config::app::{AppEnvironment, AppSection};
use crate::config::grpc::GrpcConfig;
use crate::config::runtime::RuntimeConfig;
use crate::config::worker::WorkerConfig;

#[derive(Clone, Debug)]
pub struct LogConfig {
    pub level: String,
    pub format: String,
    pub service: String,
    pub environment: String,
    pub host_id: String,
}

impl LogConfig {
    pub fn validate(&self) -> Result<(), String> {
        let level = self.level.trim().to_ascii_uppercase();
        match level.as_str() {
            "TRACE" | "DEBUG" | "INFO" | "WARN" | "WARNING" | "ERROR" | "OFF" => {}
            _ => {
                return Err(format!(
                    "invalid LOG_LEVEL value '{}', expected TRACE|DEBUG|INFO|WARN|ERROR|OFF",
                    self.level
                ));
            }
        }

        let format = self.format.trim().to_ascii_lowercase();
        match format.as_str() {
            "json" | "text" => Ok(()),
            _ => Err(format!(
                "invalid LOG_FORMAT value '{}', expected json|text",
                self.format
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub app: AppSection,
    pub agent: AgentConfig,
    pub grpc: GrpcConfig,
    pub runtime: RuntimeConfig,
    pub worker: WorkerConfig,
    pub log: LogConfig,
}

pub fn load_from_env() -> Result<AppConfig> {
    let app_name = optional_env("APP_NAME").unwrap_or_else(|| "aurora-kvm-agent".to_string());
    let node_id = required_env("APP_NODE_ID")?;

    let shutdown_timeout_secs = optional_env("SHUTDOWN_TIMEOUT_SEC")
        .unwrap_or_else(|| "15".to_string())
        .parse::<u64>()
        .context("invalid SHUTDOWN_TIMEOUT_SEC, expected unsigned integer seconds")?;

    let app = AppSection {
        name: app_name.clone(),
        environment: AppEnvironment::Prod,
        node_id: node_id.clone(),
        shutdown_timeout: Duration::from_secs(shutdown_timeout_secs),
    };
    app.validate().map_err(|e| anyhow!(e))?;

    let grpc = GrpcConfig {
        bind_addr: optional_env("GRPC_BIND_ADDR").unwrap_or_else(|| "0.0.0.0:8081".to_string()),
    };
    grpc.validate().map_err(|e| anyhow!(e))?;

    let agent = AgentConfig {
        enabled: true,
        target_addr: optional_env("AGENT_TARGET_ADDR")
            .unwrap_or_else(|| "https://127.0.0.1:9443".to_string()),
        server_name: optional_env("AGENT_SERVER_NAME").unwrap_or_default(),
        ca_path: optional_env("AGENT_CA_PATH")
            .unwrap_or_else(|| "/etc/vm-agent/tls/ca.crt".to_string()),
        cert_path: optional_env("AGENT_CERT_PATH")
            .unwrap_or_else(|| "/etc/vm-agent/tls/client.crt".to_string()),
        key_path: optional_env("AGENT_KEY_PATH")
            .unwrap_or_else(|| "/etc/vm-agent/tls/client.key".to_string()),
        bootstrap_token: optional_env("AGENT_BOOTSTRAP_TOKEN")
            .unwrap_or_else(|| "bootstrap".to_string()),
        heartbeat_interval: Duration::from_secs(
            optional_env("AGENT_HEARTBEAT_INTERVAL_SEC")
                .unwrap_or_else(|| "10".to_string())
                .parse::<u64>()
                .context(
                    "invalid AGENT_HEARTBEAT_INTERVAL_SEC, expected unsigned integer seconds",
                )?,
        ),
        hypervisor_type: optional_env("AGENT_HYPERVISOR_TYPE").unwrap_or_else(|| "kvm".to_string()),
        version: optional_env("AGENT_VERSION")
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
    };
    agent.validate().map_err(|e| anyhow!(e))?;

    let runtime = RuntimeConfig {
        driver: optional_env("RUNTIME_DRIVER").unwrap_or_else(|| "kvm".to_string()),
        redis_url: optional_env("REDIS_URL")
            .unwrap_or_else(|| "redis://127.0.0.1:6379/0".to_string()),
    };
    runtime.validate().map_err(|e| anyhow!(e))?;

    let worker = WorkerConfig {
        max_workers: optional_env("WORKER_MAX")
            .unwrap_or_else(|| "4".to_string())
            .parse::<usize>()
            .context("invalid WORKER_MAX, expected unsigned integer")?,
    };
    worker.validate().map_err(|e| anyhow!(e))?;

    let log = LogConfig {
        level: "INFO".to_string(),
        format: "json".to_string(),
        service: app_name,
        environment: "prod".to_string(),
        host_id: node_id,
    };
    log.validate().map_err(|e| anyhow!(e))?;

    Ok(AppConfig {
        app,
        agent,
        grpc,
        runtime,
        worker,
        log,
    })
}

fn required_env(key: &str) -> Result<String> {
    let value =
        env::var(key).with_context(|| format!("missing required environment variable {key}"))?;
    if value.trim().is_empty() {
        return Err(anyhow!(
            "required environment variable {key} must not be empty"
        ));
    }
    Ok(value)
}

fn optional_env(key: &str) -> Option<String> {
    match env::var(key) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}
