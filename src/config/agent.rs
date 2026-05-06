use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub enabled: bool,
    pub bootstrap_target_addr: String,
    pub runtime_target_addr: String,
    pub server_name: String,
    pub ca_path: String,
    pub cert_path: String,
    pub key_path: String,
    pub bootstrap_token: String,
    pub heartbeat_interval: Duration,
    pub version: String,
}

impl AgentConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.bootstrap_target_addr.trim().is_empty() {
            return Err("AGENT_BOOTSTRAP_TARGET_ADDR must not be empty".to_string());
        }
        if self.runtime_target_addr.trim().is_empty() {
            return Err("AGENT_RUNTIME_TARGET_ADDR must not be empty".to_string());
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
        if self.version.trim().is_empty() {
            return Err("AGENT_VERSION must not be empty".to_string());
        }
        Ok(())
    }
}
