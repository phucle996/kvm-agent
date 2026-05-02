use anyhow::{Context, Result};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc;

use crate::agent::command_handler::execute_agent_command;
use crate::model::host::HostFacts;
use crate::transport::grpc::pb::agent_registry_v1::*;

pub async fn handle_server_message(
    frame: HypervisorToAgent,
    tx: &mpsc::Sender<AgentToHypervisor>,
    _facts: &HostFacts,
    stream_id: &str,
    seq: &Arc<AtomicU64>,
) -> Result<()> {
    match frame.message {
        Some(hypervisor_to_agent::Message::RegisterAck(ack)) => {
            tracing::info!(
                component = "agent",
                operation = "register_ack",
                status = "success",
                host_id = %ack.host_id,
                node_id = %ack.node_id,
                "host registration acknowledged"
            );
        }
        Some(hypervisor_to_agent::Message::HeartbeatAck(ack)) => {
            tracing::debug!(
                component = "agent",
                operation = "heartbeat_ack",
                status = "success",
                host_id = %ack.host_id,
                ack_status = %ack.status,
                "heartbeat acknowledged"
            );
        }
        Some(hypervisor_to_agent::Message::Command(command)) => {
            tracing::info!(
                component = "agent",
                operation = "command",
                status = "received",
                command_id = %command.command_id,
                command_type = %command.r#type,
                "received hypervisor command"
            );
            let result = execute_agent_command(&command.r#type, &command.payload_json).await;
            let (status, result_json, error_message) = match result {
                Ok(value) => ("succeeded".to_string(), value, String::new()),
                Err(err) => ("failed".to_string(), "{}".to_string(), err.to_string()),
            };
            tx.send(AgentToHypervisor {
                stream_id: stream_id.to_string(),
                seq: seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                message: Some(agent_to_hypervisor::Message::CommandResult(
                    AgentCommandResult {
                        agent_id: _facts.agent_id.clone(),
                        host_id: _facts.host_id.clone(),
                        command_id: command.command_id,
                        status,
                        result_json,
                        error_message,
                        completed_at: Some(crate::agent::frames::system_time_to_timestamp(
                            SystemTime::now(),
                        )),
                    },
                )),
            })
            .await
            .context("send command result")?;
        }
        None => {}
    }
    Ok(())
}
