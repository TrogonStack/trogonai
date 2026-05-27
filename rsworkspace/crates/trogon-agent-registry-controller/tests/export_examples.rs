//! Dev-only helper to print signed example manifests. Run with:
//! `cargo test -p trogon-agent-registry-controller export_example_manifests -- --ignored --nocapture`

#[cfg(test)]
mod export {
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
    use trogon_agent_registry::LifecycleState;
    use trogon_agent_registry_controller::{
        FileManifestSigner, MANIFEST_VERSION, ManifestRecord, ManifestSigner, SignedManifest, signing_payload,
    };

    fn render(record: ManifestRecord, signer: &FileManifestSigner) -> String {
        let payload = signing_payload(&record).expect("payload");
        let signature = signer.sign_payload(&payload).expect("sign");
        let manifest = SignedManifest {
            manifest_version: MANIFEST_VERSION,
            record,
            signature,
        };
        toml::to_string_pretty(&manifest).expect("toml")
    }

    #[test]
    #[ignore = "dev helper to regenerate examples/ signed manifests"]
    fn export_example_manifests() {
        let pem = SigningKey::from_bytes(&[42u8; 32])
            .to_pkcs8_pem(LineEnding::LF)
            .expect("pem");
        let signer = FileManifestSigner::from_pem(&pem).expect("signer");
        println!("--- oncall-agent.toml ---");
        println!(
            "{}",
            render(
                ManifestRecord {
                    agent_id: "acme/oncall-agent".into(),
                    agent_version: "3.2.1".into(),
                    agent_definition_digest: "sha256:abc123def456".into(),
                    owner_team: "platform-sre".into(),
                    allowed_workloads: vec!["spiffe://acme.local/ns/prod/sa/oncall-agent".into()],
                    allowed_tools: vec![
                        "pagerduty.page".into(),
                        "github.issues.read".into(),
                        "slack.post_message".into(),
                    ],
                    allowed_audiences: vec![
                        "mcp.server.pagerduty".into(),
                        "mcp.server.github".into(),
                        "agent:acme/incident-coordinator".into(),
                    ],
                    allowed_purposes: vec!["incident.response".into(), "incident.triage".into()],
                    mesh_token_ttl_s: Some(300),
                    metadata: serde_json::json!({
                        "description": "Pages on-call and opens incident threads",
                        "repo": "https://github.com/acme/oncall-agent",
                        "contact": "platform-sre@acme.example"
                    }),
                    lifecycle_state: LifecycleState::Active,
                },
                &signer,
            )
        );
        println!("--- human-fronted-assistant.toml ---");
        println!(
            "{}",
            render(
                ManifestRecord {
                    agent_id: "acme/human-fronted-assistant".into(),
                    agent_version: "1.0.0".into(),
                    agent_definition_digest: "sha256:7890abcd".into(),
                    owner_team: "product-ops".into(),
                    allowed_workloads: vec!["sentinel:human".into()],
                    allowed_tools: vec!["linear.issues.read".into(), "slack.post_message".into()],
                    allowed_audiences: vec![
                        "mcp.server.linear".into(),
                        "agent:acme/human-fronted-assistant".into(),
                    ],
                    allowed_purposes: vec!["ops.triage".into()],
                    mesh_token_ttl_s: None,
                    metadata: serde_json::json!({
                        "description": "Human-operated assistant for ops triage",
                        "contact": "product-ops@acme.example"
                    }),
                    lifecycle_state: LifecycleState::Active,
                },
                &signer,
            )
        );
        println!("--- dev-signer.pem ---");
        print!("{}", pem.as_str());
    }
}
