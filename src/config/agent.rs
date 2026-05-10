use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub enabled: bool,
    pub bootstrap_target_addr: String,
    pub runtime_target_addr: String,
    pub runtime_target_state_path: String,
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
        let bootstrap_target = self.bootstrap_target_addr.trim().to_ascii_lowercase();
        if bootstrap_target.starts_with("https://") && self.ca_path.trim().is_empty() {
            return Err("AGENT_CA_PATH must not be empty for https bootstrap".to_string());
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
        if self.runtime_target_state_path.trim().is_empty() {
            return Err("AGENT_RUNTIME_TARGET_STATE_PATH must not be empty".to_string());
        }
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::AgentConfig;
    use std::time::Duration;

    fn valid_agent_config() -> AgentConfig {
        AgentConfig {
            enabled: true,
            bootstrap_target_addr: "http://127.0.0.1:9090".to_string(),
            runtime_target_addr: String::new(),
            runtime_target_state_path: "/tmp/runtime-target".to_string(),
            server_name: String::new(),
            ca_path: String::new(),
            cert_path: "/tmp/client.crt".to_string(),
            key_path: "/tmp/client.key".to_string(),
            bootstrap_token: "bootstrap".to_string(),
            heartbeat_interval: Duration::from_secs(10),
            telemetry_interval: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(3),
            failover_base_backoff: Duration::from_millis(200),
            failover_max_backoff: Duration::from_millis(3000),
            version: "test".to_string(),
            command_ledger_path: "/tmp/ledger.db".to_string(),
        }
    }

    #[test]
    fn allows_plaintext_bootstrap_without_ca() {
        let cfg = valid_agent_config();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn requires_ca_for_https_bootstrap() {
        let mut cfg = valid_agent_config();
        cfg.bootstrap_target_addr = "https://127.0.0.1:9443".to_string();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("AGENT_CA_PATH"));
    }

    #[test]
    fn allows_https_bootstrap_with_ca() {
        let mut cfg = valid_agent_config();
        cfg.bootstrap_target_addr = "https://127.0.0.1:9443".to_string();
        cfg.ca_path = "/tmp/ca.crt".to_string();
        assert!(cfg.validate().is_ok());
    }
}
