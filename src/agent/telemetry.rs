use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Result};
use tokio::time::{interval, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tonic::Request;

use crate::model::host::HostFacts;
use crate::service::host::usage_snapshot_gib;
use crate::transport::grpc::pb::hypervisor_telemetry_v1::hypervisor_telemetry_service_client::HypervisorTelemetryServiceClient;
use crate::transport::grpc::pb::hypervisor_telemetry_v1::{
    NodeTelemetryMetrics, PushHypervisorTelemetrySnapshotRequest, VmTelemetryMetrics,
};

pub async fn run_telemetry_loop(
    mut client: HypervisorTelemetryServiceClient<tonic::transport::Channel>,
    facts: HostFacts,
    zone: String,
    shutdown: CancellationToken,
    interval_duration: Duration,
) {
    let mut ticker = interval(interval_duration.max(Duration::from_secs(5)));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut last_cpu = read_cpu_stats().ok();
    let mut last_io = read_io_stats();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                let snapshot = build_snapshot(&facts, &zone, &mut last_cpu, &mut last_io);
                match snapshot {
                    Ok(item) => {
                        if let Err(err) = client.push_telemetry_snapshot(Request::new(item)).await {
                            tracing::warn!(
                                component = "agent",
                                operation = "telemetry_push",
                                status = "error",
                                node_id = %facts.host_id,
                                error_message = %err,
                                "failed to push telemetry snapshot"
                            );
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            component = "agent",
                            operation = "telemetry_collect",
                            status = "error",
                            node_id = %facts.host_id,
                            error_message = %err,
                            "failed to collect telemetry snapshot"
                        );
                    }
                }
            }
        }
    }
}

fn build_snapshot(
    facts: &HostFacts,
    zone: &str,
    last_cpu: &mut Option<CpuStats>,
    last_io: &mut Option<IOMetrics>,
) -> Result<PushHypervisorTelemetrySnapshotRequest> {
    let current_cpu = read_cpu_stats().ok();
    let cpu_pct = if let (Some(prev), Some(curr)) = (last_cpu.as_ref(), current_cpu.as_ref()) {
        calculate_cpu_percent(prev, curr)
    } else {
        0.0
    };
    *last_cpu = current_cpu;

    let current_io = read_io_stats();
    let (rx_bps, tx_bps, read_bps, write_bps) =
        if let (Some(prev), Some(curr)) = (last_io.as_ref(), current_io.as_ref()) {
            let elapsed = curr
                .timestamp
                .duration_since(prev.timestamp)
                .unwrap_or(Duration::from_secs(1))
                .as_secs_f64()
                .max(0.1);
            (
                ((curr.net_rx.saturating_sub(prev.net_rx)) as f64 / elapsed) as i64,
                ((curr.net_tx.saturating_sub(prev.net_tx)) as f64 / elapsed) as i64,
                ((curr.disk_read.saturating_sub(prev.disk_read)) as f64 / elapsed) as u64,
                ((curr.disk_write.saturating_sub(prev.disk_write)) as f64 / elapsed) as u64,
            )
        } else {
            (0, 0, 0, 0)
        };
    *last_io = current_io;

    let collected_at = crate::agent::frames::system_time_to_timestamp(SystemTime::now());
    let (ram_used_gib, disk_used_gib) = usage_snapshot_gib();
    let ram_used_bytes = gib_to_bytes(ram_used_gib);
    let disk_used_bytes = gib_to_bytes(disk_used_gib);
    let total_ram_bytes = facts.memory_bytes.max(0) as f64;
    let ram_used_percent = if total_ram_bytes > 0.0 {
        (ram_used_bytes / total_ram_bytes) * 100.0
    } else {
        0.0
    };
    let total_disk_bytes = facts.disk_bytes.max(0) as f64;
    let disk_used_percent = if total_disk_bytes > 0.0 {
        (disk_used_bytes / total_disk_bytes) * 100.0
    } else {
        0.0
    };
    let vms = collect_vm_metrics(&facts.host_id, zone, &collected_at)?;
    Ok(PushHypervisorTelemetrySnapshotRequest {
        schema_version: 1,
        zone: zone.trim().to_string(),
        node_id: facts.host_id.clone(),
        agent_id: facts.agent_id.clone(),
        collected_at: Some(collected_at.clone()),
        node: Some(NodeTelemetryMetrics {
            cpu_used_percent: cpu_pct,
            cpu_used_cores: (cpu_pct / 100.0) * (facts.cpu_cores as f64),
            ram_used_bytes,
            ram_used_percent,
            disk_used_bytes,
            disk_used_percent,
            network_rx_bps: rx_bps,
            network_tx_bps: tx_bps,
            disk_read_bps: read_bps,
            disk_write_bps: write_bps,
            filesystem_used_bytes: disk_used_bytes.max(0.0) as u64,
            filesystem_total_bytes: facts.disk_bytes.max(0) as u64,
            vm_count: vms.len() as u32,
            agent_healthy: true,
        }),
        vms,
    })
}

fn collect_vm_metrics(
    node_id: &str,
    zone: &str,
    collected_at: &prost_types::Timestamp,
) -> Result<Vec<VmTelemetryMetrics>> {
    let output = Command::new("virsh")
        .args(["list", "--all", "--name"])
        .output();
    let output = match output {
        Ok(item) if item.status.success() => item,
        Ok(_) | Err(_) => return Ok(Vec::new()),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut items = Vec::new();
    for line in stdout.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        let vm_id = discover_vm_uuid(name).unwrap_or_else(|| name.to_string());
        items.push(VmTelemetryMetrics {
            vm_id,
            node_id: node_id.trim().to_string(),
            zone: zone.trim().to_string(),
            power_state: discover_vm_power_state(name),
            cpu_used_percent: 0.0,
            ram_used_bytes: discover_vm_memory_bytes(name).unwrap_or(0.0),
            network_rx_bps: 0,
            network_tx_bps: 0,
            disk_read_bps: 0,
            disk_write_bps: 0,
            sampled_at: Some(collected_at.clone()),
        });
    }
    Ok(items)
}

fn discover_vm_uuid(name: &str) -> Option<String> {
    let output = Command::new("virsh")
        .args(["domuuid", name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn discover_vm_power_state(name: &str) -> String {
    let output = Command::new("virsh").args(["domstate", name]).output();
    let output = match output {
        Ok(item) if item.status.success() => item,
        _ => return "unknown".to_string(),
    };
    let value = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();
    if value.is_empty() {
        "unknown".to_string()
    } else {
        value
    }
}

fn discover_vm_memory_bytes(name: &str) -> Option<f64> {
    let output = Command::new("virsh")
        .args(["dommemstat", name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 2 {
            continue;
        }
        if parts[0] == "rss" || parts[0] == "actual" {
            if let Ok(kib) = parts[1].parse::<f64>() {
                return Some(kib * 1024.0);
            }
        }
    }
    None
}

fn gib_to_bytes(value: f64) -> f64 {
    value * 1024.0 * 1024.0 * 1024.0
}

struct IOMetrics {
    net_rx: u64,
    net_tx: u64,
    disk_read: u64,
    disk_write: u64,
    timestamp: SystemTime,
}

fn read_io_stats() -> Option<IOMetrics> {
    let mut net_rx = 0;
    let mut net_tx = 0;
    if let Ok(contents) = std::fs::read_to_string("/proc/net/dev") {
        for line in contents.lines().skip(2) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 9 {
                net_rx += parts[1].parse::<u64>().unwrap_or(0);
                net_tx += parts[9].parse::<u64>().unwrap_or(0);
            }
        }
    }

    let mut disk_read = 0;
    let mut disk_write = 0;
    if let Ok(contents) = std::fs::read_to_string("/proc/diskstats") {
        for line in contents.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 9 {
                let dev_name = parts[2];
                if dev_name.starts_with("sd")
                    || dev_name.starts_with("nvme")
                    || dev_name.starts_with("vd")
                {
                    disk_read += parts[5].parse::<u64>().unwrap_or(0) * 512;
                    disk_write += parts[9].parse::<u64>().unwrap_or(0) * 512;
                }
            }
        }
    }

    Some(IOMetrics {
        net_rx,
        net_tx,
        disk_read,
        disk_write,
        timestamp: SystemTime::now(),
    })
}

struct CpuStats {
    total: u64,
    idle: u64,
}

fn read_cpu_stats() -> Result<CpuStats> {
    let contents = std::fs::read_to_string("/proc/stat")?;
    let line = contents
        .lines()
        .next()
        .ok_or_else(|| anyhow!("empty /proc/stat"))?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 5 {
        return Err(anyhow!("invalid /proc/stat format"));
    }
    let mut total = 0u64;
    for part in parts.iter().skip(1) {
        total += part.parse::<u64>().unwrap_or(0);
    }
    let idle = parts[4].parse::<u64>().unwrap_or(0);
    Ok(CpuStats { total, idle })
}

fn calculate_cpu_percent(prev: &CpuStats, curr: &CpuStats) -> f64 {
    let total_diff = curr.total.saturating_sub(prev.total);
    let idle_diff = curr.idle.saturating_sub(prev.idle);
    if total_diff == 0 {
        return 0.0;
    }
    let used_diff = total_diff.saturating_sub(idle_diff);
    (used_diff as f64 / total_diff as f64) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::host::HostFacts;

    fn host_facts() -> HostFacts {
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
            cpu_model: "cpu".to_string(),
            memory_bytes: 8 * 1024 * 1024 * 1024,
            ram_model: "ram".to_string(),
            disk_bytes: 50 * 1024 * 1024 * 1024,
            disk_model: "disk".to_string(),
            gpu_cores: 0,
            gpu_memory_bytes: 0,
            gpu_model: "".to_string(),
            network_interfaces: Vec::new(),
        }
    }

    #[test]
    fn build_snapshot_has_required_identity() {
        let facts = host_facts();
        let mut cpu = None;
        let mut io = None;
        let snapshot = build_snapshot(&facts, "zone-a", &mut cpu, &mut io).unwrap();
        assert_eq!(snapshot.schema_version, 1);
        assert_eq!(snapshot.zone, "zone-a");
        assert_eq!(snapshot.node_id, "node-1");
        assert_eq!(snapshot.agent_id, "agent-1");
        assert!(snapshot.collected_at.is_some());
        assert!(snapshot.node.is_some());
    }
}
