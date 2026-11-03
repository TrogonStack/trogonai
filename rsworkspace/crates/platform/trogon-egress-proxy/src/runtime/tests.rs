use std::path::Path;

use super::*;
use crate::limits::Limits;
use crate::mtls::TlsFiles;
use crate::trust_domain::TrustDomain;

fn config(policy_file: &Path) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().expect("addr"),
        policy_file: policy_file.to_owned(),
        tls: TlsFiles {
            certificate_chain: "unused-server.pem".into(),
            private_key: "unused-server.key".into(),
            client_trust_anchors: "unused-ca.pem".into(),
        },
        trust_domain: TrustDomain::new("trogon.example").expect("trust domain"),
        non_tls_ports: Vec::new(),
        limits: Limits::default(),
    }
}

#[tokio::test]
async fn a_missing_policy_file_stops_startup_before_anything_else() {
    let dir = tempfile::tempdir().expect("dir");
    let error = start(config(&dir.path().join("absent.toml")))
        .await
        .err()
        .expect("refused");
    assert!(matches!(error, StartupError::Policy(SnapshotError::Unreadable { .. })));
}

#[tokio::test]
async fn an_empty_policy_file_stops_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("policy.toml");
    std::fs::write(&path, "").expect("write");
    let error = start(config(&path)).await.err().expect("refused");
    assert!(matches!(error, StartupError::Policy(SnapshotError::NoAgents)));
}

#[tokio::test]
async fn an_unparsable_policy_file_stops_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("policy.toml");
    std::fs::write(&path, "[agents.a\nallow = ").expect("write");
    let error = start(config(&path)).await.err().expect("refused");
    assert!(matches!(error, StartupError::Policy(SnapshotError::Unparsable { .. })));
}

#[tokio::test]
async fn a_valid_policy_with_missing_tls_material_still_stops_startup() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("policy.toml");
    std::fs::write(&path, "[agents.agent-1]\nallow = [\"example.com:443\"]\n").expect("write");
    let error = start(config(&path)).await.err().expect("refused");
    assert!(matches!(error, StartupError::Tls(TlsError::Unreadable { .. })));
}

#[test]
fn the_only_mode_is_enforce() {
    assert_eq!(EnforcementMode::Enforce.as_str(), "enforce");
}
