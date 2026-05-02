use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::agent::frames::{host_inventory_frame, node_metric_frame};
use crate::model::host::HostFacts;
use crate::transport::grpc::pb::agent_registry_v1::*;

pub async fn run_telemetry_loop(
    tx: mpsc::Sender<AgentToHypervisor>,
    facts: HostFacts,
    stream_id: String,
    seq: Arc<AtomicU64>,
    shutdown: CancellationToken,
    interval_duration: Duration,
) {
    let mut ticker = interval(interval_duration.max(Duration::from_secs(5)));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    if tx
        .send(host_inventory_frame(
            &facts,
            &stream_id,
            seq.fetch_add(1, Ordering::SeqCst),
        ))
        .await
        .is_err()
    {
        return;
    }

    let mut last_cpu = read_cpu_stats().ok();
    let mut last_io = read_io_stats();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                let current_cpu = read_cpu_stats().ok();
                let cpu_pct = if let (Some(prev), Some(curr)) = (&last_cpu, &current_cpu) {
                    calculate_cpu_percent(prev, curr)
                } else {
                    0.0
                };
                last_cpu = current_cpu;

                let current_io = read_io_stats();
                let (rx_bps, tx_bps, read_bps, write_bps) = if let (Some(prev), Some(curr)) = (&last_io, &current_io) {
                    let elapsed = curr.timestamp.duration_since(prev.timestamp).unwrap_or(Duration::from_secs(1)).as_secs_f64().max(0.1);
                    (
                        ((curr.net_rx.saturating_sub(prev.net_rx)) as f64 / elapsed) as u64,
                        ((curr.net_tx.saturating_sub(prev.net_tx)) as f64 / elapsed) as u64,
                        ((curr.disk_read.saturating_sub(prev.disk_read)) as f64 / elapsed) as u64,
                        ((curr.disk_write.saturating_sub(prev.disk_write)) as f64 / elapsed) as u64,
                    )
                } else {
                    (0, 0, 0, 0)
                };
                last_io = current_io;

                if tx.send(node_metric_frame(&facts, &stream_id, seq.fetch_add(1, Ordering::SeqCst), cpu_pct, rx_bps, tx_bps, read_bps, write_bps)).await.is_err() {
                    break;
                }
            }
        }
    }
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
