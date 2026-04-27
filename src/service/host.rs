use std::fs;
use std::net::UdpSocket;
use std::process::Command;

use serde_json::json;

use crate::config::AppConfig;
use crate::model::host::{HostFacts, HostRegistration};

pub fn collect_host_facts(config: &AppConfig) -> HostFacts {
    let node_id = config.app.node_id.clone();
    let hostname = discover_hostname().unwrap_or_else(|| node_id.clone());
    let private_ip = discover_private_ip().unwrap_or_else(|| "127.0.0.1".to_string());
    let cpu_cores = std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(1);
    let memory_bytes = discover_memory_bytes().unwrap_or(0);
    let disk_bytes = discover_disk_bytes().unwrap_or(0);
    let capabilities_json = json!({
        "mtls": true,
        "heartbeat": true,
    })
    .to_string();

    HostFacts {
        agent_id: node_id.clone(),
        host_id: node_id,
        hostname,
        private_ip,
        hypervisor_type: config.agent.hypervisor_type.clone(),
        agent_version: config.agent.version.clone(),
        capabilities_json,
        cpu_cores,
        memory_bytes,
        disk_bytes,
    }
}

pub fn host_registration_from_facts(facts: &HostFacts) -> HostRegistration {
    HostRegistration {
        agent_id: facts.agent_id.clone(),
        host_id: facts.host_id.clone(),
        hostname: facts.hostname.clone(),
        private_ip: facts.private_ip.clone(),
        hypervisor_type: facts.hypervisor_type.clone(),
        agent_version: facts.agent_version.clone(),
        capabilities_json: facts.capabilities_json.clone(),
        cpu_cores: facts.cpu_cores,
        memory_bytes: facts.memory_bytes,
        disk_bytes: facts.disk_bytes,
    }
}

fn discover_hostname() -> Option<String> {
    if let Ok(value) = std::env::var("HOSTNAME") {
        let trimmed = value.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }

    let output = Command::new("hostname").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let hostname = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hostname.is_empty() {
        None
    } else {
        Some(hostname)
    }
}

fn discover_private_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    if socket.connect("1.1.1.1:80").is_err() {
        return None;
    }
    let addr = socket.local_addr().ok()?;
    Some(addr.ip().to_string())
}

fn discover_memory_bytes() -> Option<i64> {
    let contents = fs::read_to_string("/proc/meminfo").ok()?;
    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("MemTotal:") {
            let amount = value.split_whitespace().next()?.parse::<i64>().ok()?;
            return Some(amount.saturating_mul(1024));
        }
    }
    None
}

fn discover_disk_bytes() -> Option<i64> {
    let output = Command::new("df").args(["-B1", "/"]).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().nth(1)?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 2 {
        return None;
    }

    fields[1].parse::<i64>().ok()
}
