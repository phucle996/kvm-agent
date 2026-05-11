use vm_agent::model::host::HostFacts;

pub fn temp_db_path(name: &str) -> String {
    let mut path = std::env::temp_dir();
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("aurora-kvm-agent-{name}-{suffix}.db"));
    path.to_string_lossy().to_string()
}

pub fn remove_file_if_exists(path: &str) {
    let _ = std::fs::remove_file(path);
}

pub fn host_facts() -> HostFacts {
    HostFacts {
        agent_id: "agent-1".to_string(),
        host_id: "node-1".to_string(),
        hostname: "node-1".to_string(),
        private_ip: "127.0.0.1".to_string(),
        hypervisor_type: "kvm".to_string(),
        agent_version: "test".to_string(),
        capabilities_json: "{}".to_string(),
        cpu_cores: 4,
        cpu_threads: 4,
        memory_bytes: 8 * 1024 * 1024 * 1024,
        disk_bytes: 50 * 1024 * 1024 * 1024,
        gpu_cores: 0,
        gpu_memory_gib: 0,
        cpu_packages: Vec::new(),
        memory_modules: Vec::new(),
        gpu_devices: Vec::new(),
        network_interfaces: Vec::new(),
    }
}
