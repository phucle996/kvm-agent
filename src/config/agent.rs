use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub enabled: bool,
    pub bootstrap_target_addr: String,
    pub server_name: String,
    pub ca_path: String,
    pub cert_path: String,
    pub key_path: String,
    pub bootstrap_token: String,
    pub heartbeat_interval: Duration,
    pub telemetry_interval: Duration,
    pub connect_timeout: Duration,
    pub failover_base_backoff: Duration,
    pub failover_max_backoff: Duration,
    pub version: String,
    pub command_ledger_path: String,
}

impl AgentConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.bootstrap_target_addr.trim().is_empty() {
            return Err("AGENT_BOOTSTRAP_TARGET_ADDR must not be empty".to_string());
        }
        if self.ca_path.trim().is_empty() {
            return Err("AGENT_CA_PATH must not be empty".to_string());
        }
        if self.cert_path.trim().is_empty() {
            return Err("AGENT_CERT_PATH must not be empty".to_string());
        }
        if self.key_path.trim().is_empty() {
            return Err("AGENT_KEY_PATH must not be empty".to_string());
        }
        if self.heartbeat_interval.is_zero() {
            return Err("AGENT_HEARTBEAT_INTERVAL_SEC must be > 0".to_string());
        }
        if self.telemetry_interval.is_zero() {
            return Err("AGENT_TELEMETRY_INTERVAL_SEC must be > 0".to_string());
        }
        if self.connect_timeout.is_zero() {
            return Err("AGENT_CONNECT_TIMEOUT_SEC must be > 0".to_string());
        }
        if self.failover_base_backoff.is_zero() {
            return Err("AGENT_FAILOVER_BASE_BACKOFF_MS must be > 0".to_string());
        }
        if self.failover_max_backoff < self.failover_base_backoff {
            return Err(
                "AGENT_FAILOVER_MAX_BACKOFF_MS must be >= AGENT_FAILOVER_BASE_BACKOFF_MS"
                    .to_string(),
            );
        }
        if self.version.trim().is_empty() {
            return Err("AGENT_VERSION must not be empty".to_string());
        }
        if self.command_ledger_path.trim().is_empty() {
            return Err("AGENT_COMMAND_LEDGER_PATH must not be empty".to_string());
        }
        Ok(())
    }
}
