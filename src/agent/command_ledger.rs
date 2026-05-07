use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct CommandLedger {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Clone, Debug)]
pub struct LedgerRecord {
    pub command_id: String,
    pub command_type: String,
    pub payload_hash: String,
    pub resource_key: Option<String>,
    pub status: String,
    pub result_json: String,
    pub error_message: String,
}

impl CommandLedger {
    pub fn open(path: &str) -> Result<Self> {
        let path = path.trim();
        if path.is_empty() {
            return Err(anyhow!("agent command ledger path is required"));
        }
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create command ledger dir {}", parent.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("open command ledger {}", path))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS command_ledger (
                command_id TEXT PRIMARY KEY,
                command_type TEXT NOT NULL,
                payload_hash TEXT NOT NULL,
                resource_key TEXT,
                status TEXT NOT NULL,
                result_json TEXT NOT NULL DEFAULT '{}',
                error_message TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                started_at INTEGER NOT NULL,
                completed_at INTEGER,
                updated_at INTEGER NOT NULL
            );",
        )?;
        let ledger = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        ledger.mark_stale_running_failed()?;
        Ok(ledger)
    }

    pub fn begin_or_get(
        &self,
        command_id: &str,
        command_type: &str,
        payload_json: &str,
    ) -> Result<BeginOrGet> {
        let command_id = command_id.trim();
        if command_id.is_empty() {
            return Err(anyhow!("command_id is required"));
        }
        let command_type = command_type.trim();
        let payload_hash = hash_payload(payload_json);
        let resource_key = extract_resource_key(payload_json);
        let now = unix_now();
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow!("command ledger lock poisoned"))?;
        if let Some(record) = load_record(&conn, command_id)? {
            if record.command_type != command_type || record.payload_hash != payload_hash {
                return Ok(BeginOrGet::PayloadMismatch(record));
            }
            return Ok(BeginOrGet::Existing(record));
        }
        conn.execute(
            "INSERT INTO command_ledger (command_id, command_type, payload_hash, resource_key, status, result_json, error_message, created_at, started_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'running', '{}', '', ?5, ?5, ?5)",
            params![command_id, command_type, payload_hash, resource_key, now],
        )?;
        let record = load_record(&conn, command_id)?
            .ok_or_else(|| anyhow!("command ledger insert verification failed"))?;
        Ok(BeginOrGet::New(record))
    }

    pub fn complete(
        &self,
        command_id: &str,
        status: &str,
        result_json: &str,
        error_message: &str,
    ) -> Result<()> {
        let now = unix_now();
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow!("command ledger lock poisoned"))?;
        conn.execute(
            "UPDATE command_ledger SET status = ?2, result_json = ?3, error_message = ?4, completed_at = ?5, updated_at = ?5 WHERE command_id = ?1",
            params![command_id.trim(), status.trim(), normalize_result_json(result_json), error_message.trim(), now],
        )?;
        Ok(())
    }

    fn mark_stale_running_failed(&self) -> Result<()> {
        let now = unix_now();
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow!("command ledger lock poisoned"))?;
        conn.execute(
            "UPDATE command_ledger SET status = 'failed', error_message = 'agent_restart_before_completion', completed_at = ?1, updated_at = ?1 WHERE status = 'running'",
            params![now],
        )?;
        Ok(())
    }
}

pub enum BeginOrGet {
    New(LedgerRecord),
    Existing(LedgerRecord),
    PayloadMismatch(LedgerRecord),
}

fn load_record(conn: &Connection, command_id: &str) -> Result<Option<LedgerRecord>> {
    conn.query_row(
        "SELECT command_id, command_type, payload_hash, resource_key, status, result_json, error_message FROM command_ledger WHERE command_id = ?1",
        params![command_id.trim()],
        |row| {
            Ok(LedgerRecord {
                command_id: row.get(0)?,
                command_type: row.get(1)?,
                payload_hash: row.get(2)?,
                resource_key: row.get(3)?,
                status: row.get(4)?,
                result_json: row.get(5)?,
                error_message: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn hash_payload(payload_json: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload_json.trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

fn extract_resource_key(payload_json: &str) -> Option<String> {
    let payload: Value = serde_json::from_str(payload_json).ok()?;
    for key in ["vps_id", "vm_id", "instance_id", "domain_id", "id"] {
        if let Some(value) = payload.get(key).and_then(|item| item.as_str()) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn normalize_result_json(value: &str) -> String {
    if value.trim().is_empty() {
        return "{}".to_string();
    }
    value.to_string()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db_path(name: &str) -> String {
        let mut path = std::env::temp_dir();
        path.push(format!("aurora-kvm-agent-{name}-{}.db", unix_now()));
        path.to_string_lossy().to_string()
    }

    #[test]
    fn ledger_returns_existing_after_success() {
        let path = temp_db_path("success");
        let ledger = CommandLedger::open(&path).unwrap();
        match ledger
            .begin_or_get("cmd-1", "start", r#"{"vps_id":"vps-1"}"#)
            .unwrap()
        {
            BeginOrGet::New(_) => {}
            _ => panic!("expected new record"),
        }
        ledger
            .complete("cmd-1", "succeeded", r#"{"ok":true}"#, "")
            .unwrap();
        match ledger
            .begin_or_get("cmd-1", "start", r#"{"vps_id":"vps-1"}"#)
            .unwrap()
        {
            BeginOrGet::Existing(record) => {
                assert_eq!(record.status, "succeeded");
                assert_eq!(record.result_json, r#"{"ok":true}"#);
            }
            _ => panic!("expected existing record"),
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ledger_marks_stale_running_failed_on_reopen() {
        let path = temp_db_path("stale-running");
        let ledger = CommandLedger::open(&path).unwrap();
        match ledger
            .begin_or_get("cmd-1", "start", r#"{"vps_id":"vps-1"}"#)
            .unwrap()
        {
            BeginOrGet::New(_) => {}
            _ => panic!("expected new record"),
        }
        drop(ledger);

        let reopened = CommandLedger::open(&path).unwrap();
        match reopened
            .begin_or_get("cmd-1", "start", r#"{"vps_id":"vps-1"}"#)
            .unwrap()
        {
            BeginOrGet::Existing(record) => {
                assert_eq!(record.status, "failed");
                assert_eq!(record.error_message, "agent_restart_before_completion");
            }
            _ => panic!("expected existing failed record"),
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ledger_detects_command_type_mismatch() {
        let path = temp_db_path("type-mismatch");
        let ledger = CommandLedger::open(&path).unwrap();
        let _ = ledger
            .begin_or_get("cmd-1", "start", r#"{"vps_id":"vps-1"}"#)
            .unwrap();
        match ledger
            .begin_or_get("cmd-1", "stop", r#"{"vps_id":"vps-1"}"#)
            .unwrap()
        {
            BeginOrGet::PayloadMismatch(_) => {}
            _ => panic!("expected command type mismatch"),
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ledger_detects_payload_mismatch() {
        let path = temp_db_path("mismatch");
        let ledger = CommandLedger::open(&path).unwrap();
        let _ = ledger
            .begin_or_get("cmd-1", "start", r#"{"vps_id":"vps-1"}"#)
            .unwrap();
        match ledger
            .begin_or_get("cmd-1", "start", r#"{"vps_id":"vps-2"}"#)
            .unwrap()
        {
            BeginOrGet::PayloadMismatch(_) => {}
            _ => panic!("expected payload mismatch"),
        }
        let _ = std::fs::remove_file(path);
    }
}
