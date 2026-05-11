mod support;

use support::host_facts;
use vm_agent::agent::telemetry::build_snapshot_for_test;

#[test]
fn build_snapshot_has_required_identity() {
    let facts = host_facts();
    let snapshot = build_snapshot_for_test(&facts, "zone-a").unwrap();
    assert_eq!(snapshot.schema_version, 1);
    assert_eq!(snapshot.zone, "zone-a");
    assert_eq!(snapshot.node_id, "node-1");
    assert_eq!(snapshot.agent_id, "agent-1");
    assert!(snapshot.collected_at.is_some());
    assert!(snapshot.node.is_some());
}
