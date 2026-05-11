mod support;

use support::{remove_file_if_exists, temp_db_path};
use vm_agent::agent::command_ledger::{BeginOrGet, CommandLedger};

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
    remove_file_if_exists(&path);
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
    remove_file_if_exists(&path);
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
    remove_file_if_exists(&path);
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
    remove_file_if_exists(&path);
}
