use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct HostFacts {
    pub agent_id: String,
    pub host_id: String,
    pub hostname: String,
    pub private_ip: String,
    pub hypervisor_type: String,
    pub agent_version: String,
    pub capabilities_json: String,
    pub cpu_cores: i32,
    pub memory_bytes: i64,
    pub disk_bytes: i64,
}

#[derive(Clone, Debug)]
pub struct HostRegistration {
    pub agent_id: String,
    pub host_id: String,
    pub hostname: String,
    pub private_ip: String,
    pub hypervisor_type: String,
    pub agent_version: String,
    pub capabilities_json: String,
    pub cpu_cores: i32,
    pub memory_bytes: i64,
    pub disk_bytes: i64,
}

#[derive(Clone, Debug)]
pub struct AgentIdentityState {
    pub client_cert_pem: Vec<u8>,
    pub client_key_pem: Vec<u8>,
    pub ca_bundle_pem: Vec<u8>,
    pub cert_not_after: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HeartbeatState {
    pub agent_id: String,
    pub host_id: String,
    pub status: String,
    pub last_seen_at: SystemTime,
}
