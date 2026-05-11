use std::time::Duration;

use vm_agent::config::agent::AgentConfig;

fn valid_agent_config() -> AgentConfig {
    AgentConfig {
        enabled: true,
        bootstrap_target_addr: "http://127.0.0.1:9090".to_string(),
        runtime_target_addr: String::new(),
        runtime_target_state_path: "/tmp/runtime-target".to_string(),
        server_name: String::new(),
        ca_path: String::new(),
        cert_path: "/tmp/client.crt".to_string(),
        key_path: "/tmp/client.key".to_string(),
        bootstrap_token: "bootstrap".to_string(),
        heartbeat_interval: Duration::from_secs(10),
        telemetry_interval: Duration::from_secs(15),
        connect_timeout: Duration::from_secs(3),
        failover_base_backoff: Duration::from_millis(200),
        failover_max_backoff: Duration::from_millis(3000),
        version: "test".to_string(),
        command_ledger_path: "/tmp/ledger.db".to_string(),
    }
}

#[test]
fn allows_plaintext_bootstrap_without_ca() {
    let cfg = valid_agent_config();
    assert!(cfg.validate().is_ok());
}

#[test]
fn requires_ca_for_https_bootstrap() {
    let mut cfg = valid_agent_config();
    cfg.bootstrap_target_addr = "https://127.0.0.1:9443".to_string();
    let err = cfg.validate().unwrap_err();
    assert!(err.contains("AGENT_CA_PATH"));
}

#[test]
fn allows_https_bootstrap_with_ca() {
    let mut cfg = valid_agent_config();
    cfg.bootstrap_target_addr = "https://127.0.0.1:9443".to_string();
    cfg.ca_path = "/tmp/ca.crt".to_string();
    assert!(cfg.validate().is_ok());
}
