use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub enabled: bool,
    pub bootstrap_target_addr: String,
    pub runtime_target_addr: String,
    pub runtime_target_addrs: Vec<String>,
    pub server_name: String,
    pub ca_path: String,
    pub cert_path: String,
    pub key_path: String,
    pub bootstrap_token: String,
    pub heartbeat_interval: Duration,
    pub connect_timeout: Duration,
    pub failover_base_backoff: Duration,
    pub failover_max_backoff: Duration,
    pub version: String,
}

impl AgentConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.bootstrap_target_addr.trim().is_empty() {
            return Err("AGENT_BOOTSTRAP_TARGET_ADDR must not be empty".to_string());
        }
        if self.runtime_targets().is_empty() {
            return Err(
                "AGENT_RUNTIME_TARGET_ADDR or AGENT_RUNTIME_TARGET_ADDRS must provide at least one target"
                    .to_string(),
            );
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
        Ok(())
    }

    pub fn runtime_targets(&self) -> Vec<String> {
        let mut targets: Vec<String> = self
            .runtime_target_addrs
            .iter()
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect();
        if targets.is_empty() && !self.runtime_target_addr.trim().is_empty() {
            targets.push(self.runtime_target_addr.trim().to_string());
        }
        targets
    }
}

#[cfg(test)]
mod tests {
    use super::AgentConfig;
    use std::time::Duration;

    fn sample() -> AgentConfig {
        AgentConfig {
            enabled: true,
            bootstrap_target_addr: "https://cp:9443".to_string(),
            runtime_target_addr: "https://dp-1:50051".to_string(),
            runtime_target_addrs: vec![],
            server_name: String::new(),
            ca_path: "/tmp/ca".to_string(),
            cert_path: "/tmp/cert".to_string(),
            key_path: "/tmp/key".to_string(),
            bootstrap_token: "token".to_string(),
            heartbeat_interval: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(3),
            failover_base_backoff: Duration::from_millis(200),
            failover_max_backoff: Duration::from_secs(3),
            version: "0.1.0".to_string(),
        }
    }

    #[test]
    fn runtime_targets_fallback_to_single_target() {
        let cfg = sample();
        assert_eq!(
            cfg.runtime_targets(),
            vec!["https://dp-1:50051".to_string()]
        );
    }

    #[test]
    fn runtime_targets_prefer_explicit_list() {
        let mut cfg = sample();
        cfg.runtime_target_addrs = vec![
            " https://dp-1:50051 ".to_string(),
            "".to_string(),
            "https://dp-2:50051".to_string(),
        ];
        assert_eq!(
            cfg.runtime_targets(),
            vec![
                "https://dp-1:50051".to_string(),
                "https://dp-2:50051".to_string()
            ]
        );
    }
}
