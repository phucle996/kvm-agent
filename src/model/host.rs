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
    pub cpu_threads: i32,
    pub memory_bytes: i64,
    pub disk_bytes: i64,
    pub gpu_cores: i32,
    pub gpu_memory_gib: i64,
    pub cpu_packages: Vec<CPUPackage>,
    pub memory_modules: Vec<MemoryModule>,
    pub gpu_devices: Vec<GPUDevice>,
    pub network_interfaces: Vec<NetworkInterface>,
}

#[derive(Clone, Debug)]
pub struct CPUPackage {
    pub package_index: i32,
    pub model: String,
    pub cores: i32,
    pub threads: i32,
}

#[derive(Clone, Debug)]
pub struct MemoryModule {
    pub slot_index: i32,
    pub model: String,
    pub size_gib: i32,
}

#[derive(Clone, Debug)]
pub struct GPUDevice {
    pub device_index: i32,
    pub model: String,
    pub memory_gib: i64,
    pub core_count: i32,
}

#[derive(Clone, Debug)]
pub struct NetworkInterface {
    pub name: String,
    pub mac_address: String,
    pub ipv4_address: String,
    pub ipv6_address: String,
    pub speed_mbps: i32,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct StoragePool {
    pub name: String,
    pub driver: String,
    pub path: String,
    pub total_bytes: i64,
    pub used_bytes: i64,
    pub status: String,
    pub metadata_json: String,
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
}

#[derive(Clone, Debug)]
pub struct AgentIdentityState {
    pub client_cert_pem: Vec<u8>,
    pub client_key_pem: Vec<u8>,
    pub ca_bundle_pem: Vec<u8>,
    pub cert_not_after: Option<String>,
    pub runtime_target_addr: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HeartbeatState {
    pub agent_id: String,
    pub host_id: String,
    pub status: String,
    pub last_seen_at: SystemTime,
}
