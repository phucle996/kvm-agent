use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub enabled: bool,
    pub target_addr: String,
    pub server_name: String,
    pub ca_path: String,
    pub cert_path: String,
    pub key_path: String,
    pub bootstrap_token: String,
    pub heartbeat_interval: Duration,
    pub hypervisor_type: String,
    pub version: String,
}

impl AgentConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.target_addr.trim().is_empty() {
            return Err("AGENT_TARGET_ADDR must not be empty".to_string());
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
        if self.hypervisor_type.trim().is_empty() {
            return Err("AGENT_HYPERVISOR_TYPE must not be empty".to_string());
        }
        if self.version.trim().is_empty() {
            return Err("AGENT_VERSION must not be empty".to_string());
        }
        Ok(())
    }
}
