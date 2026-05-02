use crate::model::host::HostFacts;
use crate::service::host::usage_snapshot_gib;
use crate::transport::grpc::pb::agent_registry_v1::*;
use prost_types::Timestamp;
use std::time::SystemTime;

pub fn system_time_to_timestamp(t: SystemTime) -> Timestamp {
    let duration = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    Timestamp {
        seconds: duration.as_secs() as i64,
        nanos: duration.subsec_nanos() as i32,
    }
}

pub fn register_frame(facts: &HostFacts, stream_id: &str, seq: u64) -> AgentToHypervisor {
    AgentToHypervisor {
        stream_id: stream_id.to_string(),
        seq,
        message: Some(agent_to_hypervisor::Message::RegisterHost(RegisterHost {
            agent_id: facts.agent_id.clone(),
            host_id: facts.host_id.clone(),
            hostname: facts.hostname.clone(),
            private_ip: facts.private_ip.clone(),
            hypervisor_type: facts.hypervisor_type.clone(),
            agent_version: facts.agent_version.clone(),
            capabilities_json: facts.capabilities_json.clone(),
            cpu_cores: facts.cpu_cores,
            cpu_threads: facts.cpu_threads,
            memory_bytes: facts.memory_bytes,
            disk_bytes: facts.disk_bytes,
            gpu_cores: facts.gpu_cores,
            gpu_memory_bytes: facts.gpu_memory_bytes,
            cpu_model: facts.cpu_model.clone(),
            ram_model: facts.ram_model.clone(),
            disk_model: facts.disk_model.clone(),
            gpu_model: facts.gpu_model.clone(),
        })),
    }
}

pub fn node_metric_frame(
    facts: &HostFacts,
    stream_id: &str,
    seq: u64,
    cpu_used_percent: f64,
    network_rx_bps: u64,
    network_tx_bps: u64,
    disk_read_bps: u64,
    disk_write_bps: u64,
) -> AgentToHypervisor {
    let (ram_used_gib, ssd_used_gib) = usage_snapshot_gib();

    let cpu_used_cores = (cpu_used_percent / 100.0) * (facts.cpu_cores as f64);

    let total_ram_gib = facts.memory_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    let ram_used_percent = if total_ram_gib > 0.0 {
        (ram_used_gib / total_ram_gib) * 100.0
    } else {
        0.0
    };

    let total_ssd_gib = facts.disk_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    let ssd_used_percent = if total_ssd_gib > 0.0 {
        (ssd_used_gib / total_ssd_gib) * 100.0
    } else {
        0.0
    };

    AgentToHypervisor {
        stream_id: stream_id.to_string(),
        seq,
        message: Some(agent_to_hypervisor::Message::NodeMetric(NodeMetricSample {
            agent_id: facts.agent_id.clone(),
            host_id: facts.host_id.clone(),
            cpu_used_percent,
            cpu_used_cores,
            ram_used_gib,
            ram_used_percent,
            ssd_used_gib,
            ssd_used_percent,
            gpu_used_percent: 0.0,
            gpu_used_gib: 0.0,
            network_rx_bps: network_rx_bps as i64,
            network_tx_bps: network_tx_bps as i64,
            disk_read_bps,
            disk_write_bps,
            sampled_at: Some(system_time_to_timestamp(SystemTime::now())),
        })),
    }
}

pub fn host_inventory_frame(facts: &HostFacts, stream_id: &str, seq: u64) -> AgentToHypervisor {
    let network_interfaces = facts
        .network_interfaces
        .iter()
        .map(|iface| NetworkInterfaceInventory {
            name: iface.name.clone(),
            mac_address: iface.mac_address.clone(),
            ipv4_address: iface.ipv4_address.clone(),
            ipv6_address: iface.ipv6_address.clone(),
            speed_mbps: iface.speed_mbps,
            status: iface.status.clone(),
            metadata_json: "{}".to_string(),
        })
        .collect();

    AgentToHypervisor {
        stream_id: stream_id.to_string(),
        seq,
        message: Some(agent_to_hypervisor::Message::HostInventory(HostInventory {
            agent_id: facts.agent_id.clone(),
            host_id: facts.host_id.clone(),
            storage_pools: crate::service::host::discover_storage_pools()
                .into_iter()
                .map(|p| StoragePoolInventory {
                    name: p.name,
                    driver: p.driver,
                    path: p.path,
                    total_bytes: p.total_bytes,
                    used_bytes: p.used_bytes,
                    status: p.status,
                    metadata_json: p.metadata_json,
                })
                .collect(),
            network_interfaces,
            collected_at: Some(system_time_to_timestamp(SystemTime::now())),
        })),
    }
}
