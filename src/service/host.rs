use std::fs;
use std::net::UdpSocket;
use std::process::Command;

use serde_json::json;

use crate::config::AppConfig;
use crate::model::host::{HostFacts, HostRegistration, NetworkInterface, StoragePool};

pub fn collect_host_facts(config: &AppConfig) -> HostFacts {
    let node_id = config.app.node_id.clone();
    let hostname = discover_hostname().unwrap_or_else(|| node_id.clone());
    let network_interfaces = discover_network_interfaces();
    let private_ip = network_interfaces
        .iter()
        .find(|iface| iface.name != "lo" && !iface.ipv4_address.is_empty())
        .map(|iface| iface.ipv4_address.clone())
        .unwrap_or_else(|| discover_private_ip().unwrap_or_else(|| "127.0.0.1".to_string()));
    let (cpu_cores, cpu_threads) = discover_cpu_specs();
    let cpu_model = discover_cpu_model();
    let memory_bytes = discover_memory_bytes().unwrap_or(0);
    let ram_model = discover_ram_model();
    let disk_bytes = discover_disk_bytes().unwrap_or(0);
    let disk_model = discover_disk_model();
    let gpu_model = discover_gpu_model();
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
        hypervisor_type: "kvm".to_string(),
        agent_version: config.agent.version.clone(),
        capabilities_json,
        cpu_cores,
        cpu_threads,
        cpu_model,
        memory_bytes,
        ram_model,
        disk_bytes,
        disk_model,
        gpu_cores: 0,
        gpu_memory_bytes: 0,
        gpu_model,
        network_interfaces,
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
        cpu_threads: facts.cpu_threads,
        cpu_model: facts.cpu_model.clone(),
        memory_bytes: facts.memory_bytes,
        ram_model: facts.ram_model.clone(),
        disk_bytes: facts.disk_bytes,
        disk_model: facts.disk_model.clone(),
        gpu_cores: facts.gpu_cores,
        gpu_memory_bytes: facts.gpu_memory_bytes,
        gpu_model: facts.gpu_model.clone(),
        network_interfaces: facts.network_interfaces.clone(),
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
            let amount_kb = value.split_whitespace().next()?.parse::<i64>().ok()?;
            let bytes = amount_kb.saturating_mul(1024);
            return Some(round_to_physical_ram(bytes));
        }
    }
    None
}

fn round_to_physical_ram(bytes: i64) -> i64 {
    let gib = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    let rounded_gib = if gib <= 1.0 {
        1.0
    } else if gib <= 2.0 {
        2.0
    } else if gib <= 4.0 {
        4.0
    } else if gib <= 8.0 {
        8.0
    } else if gib <= 12.0 {
        12.0
    } else if gib <= 16.0 {
        16.0
    } else if gib <= 24.0 {
        24.0
    } else if gib <= 32.0 {
        32.0
    } else if gib <= 64.0 {
        64.0
    } else if gib <= 128.0 {
        128.0
    } else {
        gib.ceil()
    };
    (rounded_gib * 1024.0 * 1024.0 * 1024.0) as i64
}

fn discover_cpu_specs() -> (i32, i32) {
    let contents = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let mut processors = 0;
    let mut cpu_cores = 0;

    for line in contents.lines() {
        if line.starts_with("processor") {
            processors += 1;
        } else if line.starts_with("cpu cores") {
            if let Some(val) = line.split(':').nth(1) {
                if let Ok(c) = val.trim().parse::<i32>() {
                    // We only take the first one, assuming uniform cores per socket
                    if cpu_cores == 0 {
                        cpu_cores = c;
                    }
                }
            }
        }
    }

    if cpu_cores == 0 {
        cpu_cores = processors;
    }

    (cpu_cores, processors)
}

fn discover_cpu_model() -> String {
    let Ok(contents) = fs::read_to_string("/proc/cpuinfo") else {
        return "Unknown CPU".to_string();
    };
    for line in contents.lines() {
        if line.starts_with("model name") {
            if let Some(pos) = line.find(':') {
                return line[pos + 1..].trim().to_string();
            }
        }
    }
    "Unknown CPU".to_string()
}

fn discover_disk_model() -> String {
    if let Ok(entries) = fs::read_dir("/sys/block") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("sd") || name_str.starts_with("nvme") || name_str.starts_with("vd") {
                let model_path = format!("/sys/block/{}/device/model", name_str);
                if let Ok(model) = fs::read_to_string(model_path) {
                    let trimmed = model.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
                // Try NVMe specific
                let model_path = format!("/sys/class/block/{}/device/model", name_str);
                if let Ok(model) = fs::read_to_string(model_path) {
                    let trimmed = model.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
            }
        }
    }
    "Generic System Disk".to_string()
}

fn discover_ram_model() -> String {
    "Standard Memory Module".to_string()
}

fn discover_gpu_model() -> String {
    if let Ok(output) = Command::new("lspci").output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains("VGA compatible controller") || line.contains("NVIDIA") || line.contains("AMD") {
                if let Some(pos) = line.find(':') {
                    let model = line[pos + 1..].trim();
                    if !model.is_empty() {
                        return model.to_string();
                    }
                }
            }
        }
    }
    "Internal Graphics".to_string()
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

fn discover_network_interfaces() -> Vec<NetworkInterface> {
    let mut interfaces = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return interfaces;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();

        let mac_address = fs::read_to_string(path.join("address"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let status = fs::read_to_string(path.join("operstate"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let speed_mbps = fs::read_to_string(path.join("speed"))
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .map(|s| s.max(0))
            .unwrap_or(0);

        // Simple way to get IPv4 using 'ip' command if available
        let ipv4_address = get_ip_for_interface(&name, false);
        let ipv6_address = get_ip_for_interface(&name, true);

        interfaces.push(NetworkInterface {
            name,
            mac_address,
            ipv4_address,
            ipv6_address,
            speed_mbps,
            status,
        });
    }
    interfaces
}

fn get_ip_for_interface(name: &str, ipv6: bool) -> String {
    let arg = if ipv6 { "-6" } else { "-4" };
    let Ok(output) = Command::new("ip")
        .args([arg, "addr", "show", name])
        .output()
    else {
        return String::new();
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("inet ") || trimmed.starts_with("inet6 ") {
            if let Some(addr_part) = trimmed.split_whitespace().nth(1) {
                return addr_part.split('/').next().unwrap_or("").to_string();
            }
        }
    }
    String::new()
}

pub fn discover_storage_pools() -> Vec<StoragePool> {
    let mut pools = Vec::new();
    let contents = fs::read_to_string("/proc/mounts").unwrap_or_default();
    
    for line in contents.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 { continue; }
        let device = fields[0];
        let path = fields[1];
        let fs_type = fields[2];

        // Filter for physical partitions that are commonly used for storage
        if (device.starts_with("/dev/sd") || device.starts_with("/dev/nvme") || device.starts_with("/dev/vd") || device.starts_with("/dev/mapper/"))
           && (fs_type == "ext4" || fs_type == "xfs" || fs_type == "btrfs") {
            
            let name = if path == "/" { 
                "root".to_string() 
            } else { 
                path.trim_start_matches('/').replace('/', "-") 
            };
            
            if let Ok(output) = Command::new("df").args(["-B1", path]).output() {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let mut lines = stdout.lines();
                    lines.next(); // skip header
                    if let Some(df_line) = lines.next() {
                         let df_fields: Vec<&str> = df_line.split_whitespace().collect();
                         // Handle cases where output might be wrapped
                         let (total_idx, used_idx) = if df_fields.len() >= 3 {
                             (1, 2)
                         } else if let Some(wrapped_line) = lines.next() {
                             let wrapped_fields: Vec<&str> = wrapped_line.split_whitespace().collect();
                             if wrapped_fields.len() >= 2 {
                                 (0, 1) // In wrapped case, the next line starts with numbers
                             } else {
                                 continue;
                             }
                         } else {
                             continue;
                         };

                         let total = df_fields.get(total_idx).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                         let used = df_fields.get(used_idx).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                         
                         if total > 0 {
                             pools.push(StoragePool {
                                 name,
                                 driver: "dir".to_string(),
                                 path: path.to_string(),
                                 total_bytes: total,
                                 used_bytes: used,
                                 status: "active".to_string(),
                                 metadata_json: json!({ "device": device }).to_string(),
                             });
                         }
                    }
                }
            }
        }
    }
    pools
}

pub fn usage_snapshot_gib() -> (f64, f64) {
    let ram_used = read_ram_used_gib().unwrap_or(0.0);
    let ssd_used = read_ssd_used_gib().unwrap_or(0.0);
    (ram_used, ssd_used)
}

fn read_ram_used_gib() -> Option<f64> {
    let contents = fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0.0;
    let mut available = 0.0;
    for line in contents.lines() {
        if let Some(val) = line.strip_prefix("MemTotal:") {
            total = val.split_whitespace().next()?.parse::<f64>().ok()? / 1024.0 / 1024.0;
        }
        if let Some(val) = line.strip_prefix("MemAvailable:") {
            available = val.split_whitespace().next()?.parse::<f64>().ok()? / 1024.0 / 1024.0;
        }
    }
    if total > 0.0 {
        Some(total - available)
    } else {
        None
    }
}

fn read_ssd_used_gib() -> Option<f64> {
    let output = Command::new("df").args(["-B1", "/"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().nth(1)?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 3 {
        return None;
    }
    let used_bytes = fields[2].parse::<f64>().ok()?;
    Some(used_bytes / 1024.0 / 1024.0 / 1024.0)
}
