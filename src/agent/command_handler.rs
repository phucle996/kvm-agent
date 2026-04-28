use anyhow::{anyhow, Result};
use serde_json::{json, Value};

const SUPPORTED_COMMANDS: &[&str] = &[
    "start",
    "stop",
    "restart",
    "reboot",
    "shutdown",
    "poweroff",
    "pause",
    "resume",
    "delete",
    "destroy",
    "snapshot_create",
    "snapshot_delete",
];

pub async fn execute_agent_command(command_type: &str, payload_json: &str) -> Result<String> {
    let normalized = command_type.trim().to_lowercase();
    if normalized.is_empty() {
        return Err(anyhow!("agent command type is required"));
    }

    let payload = parse_payload(payload_json)?;
    let target = extract_target(&payload);

    if !SUPPORTED_COMMANDS.contains(&normalized.as_str()) {
        return Err(anyhow!("agent command type '{}' is unsupported", normalized));
    }

    Err(anyhow!(
        "agent command '{}' for target '{}' is not implemented by kvm runtime yet",
        normalized,
        target
    ))
}

fn parse_payload(payload_json: &str) -> Result<Value> {
    if payload_json.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(payload_json)
        .map_err(|err| anyhow!("invalid command payload json: {err}"))
}

fn extract_target(payload: &Value) -> String {
    for key in ["vps_id", "vm_id", "instance_id", "domain_id", "id"] {
        if let Some(value) = payload.get(key).and_then(|value| value.as_str()) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "unknown-target".to_string()
}
