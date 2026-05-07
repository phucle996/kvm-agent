use anyhow::{Context, Result};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc;

use crate::agent::command_handler::execute_agent_command;
use crate::agent::command_ledger::{BeginOrGet, CommandLedger};
use crate::model::host::HostFacts;
use crate::transport::grpc::pb::agent_registry_v1::*;

pub async fn handle_server_message(
    frame: HypervisorToAgent,
    tx: &mpsc::Sender<AgentToHypervisor>,
    _facts: &HostFacts,
    stream_id: &str,
    seq: &Arc<AtomicU64>,
    command_ledger: &CommandLedger,
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
            let (status, result_json, error_message) = match command_ledger.begin_or_get(
                &command.command_id,
                &command.r#type,
                &command.payload_json,
            )? {
                BeginOrGet::New(_) => {
                    let result =
                        execute_agent_command(&command.r#type, &command.payload_json).await;
                    match result {
                        Ok(value) => {
                            command_ledger.complete(
                                &command.command_id,
                                "succeeded",
                                &value,
                                "",
                            )?;
                            ("succeeded".to_string(), value, String::new())
                        }
                        Err(err) => {
                            command_ledger.complete(
                                &command.command_id,
                                "failed",
                                "{}",
                                &err.to_string(),
                            )?;
                            ("failed".to_string(), "{}".to_string(), err.to_string())
                        }
                    }
                }
                BeginOrGet::Existing(record) => match record.status.as_str() {
                    "running" => ("running".to_string(), "{}".to_string(), String::new()),
                    "succeeded" => (
                        "succeeded".to_string(),
                        record.result_json,
                        record.error_message,
                    ),
                    _ => (
                        "failed".to_string(),
                        record.result_json,
                        record.error_message,
                    ),
                },
                BeginOrGet::PayloadMismatch(_record) => (
                    "failed".to_string(),
                    "{}".to_string(),
                    "command_id_payload_mismatch".to_string(),
                ),
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
