mod support;

use vm_agent::agent::bootstrap::normalize_bootstrap_endpoint;

#[test]
fn bootstrap_endpoint_defaults_to_plaintext_when_scheme_missing() {
    assert_eq!(
        normalize_bootstrap_endpoint("controlplane.local:9090").unwrap(),
        "http://controlplane.local:9090"
    );
}

#[test]
fn bootstrap_endpoint_preserves_plaintext() {
    assert_eq!(
        normalize_bootstrap_endpoint("http://127.0.0.1:9090").unwrap(),
        "http://127.0.0.1:9090"
    );
}

#[test]
fn bootstrap_endpoint_preserves_tls() {
    assert_eq!(
        normalize_bootstrap_endpoint("https://127.0.0.1:9443").unwrap(),
        "https://127.0.0.1:9443"
    );
}
