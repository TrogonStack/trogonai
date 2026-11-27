use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{BootMode, NetworkPolicy, PrivilegedAccess};
use trogon_sandbox_providers::{
    CommitSha, EgressPolicy, GitProvider, ImageRef, NoopArtifactSink, RepoRef, SandboxSpec,
};

pub fn spec() -> SandboxSpec {
    SandboxSpec {
        image: ImageRef::new("alpine:3"),
        cpu: 1.0,
        memory_mib: 128,
        timeout: Duration::from_secs(30),
        repo: RepoRef {
            provider: GitProvider::GitHub,
            owner: "owner".to_string(),
            name: "repo".to_string(),
        },
        resolved_sha: CommitSha::placeholder(),
        base_sha: None,
        tools: vec![],
        artifact_sink: Arc::new(NoopArtifactSink),
        egress_policy: EgressPolicy::DenyAll,
        network_policy: None,
        clone: None,
        host_mount: None,
        extra_binds: vec![],
        disk_mib: None,
        boot_mode: BootMode::default(),
        working_dir: None,
        user: None,
        privileged: PrivilegedAccess::Denied,
        exposed_ports: vec![],
        env: BTreeMap::new(),
        lease_id: None,
    }
}

/// A `UserDefined` policy whose rule forwards requests, so the sidecar terminates TLS.
pub fn intercepting_policy() -> Result<NetworkPolicy, serde_json::Error> {
    serde_json::from_str(
        r#"{"user_defined":{"rules":[{"domain":"api.example.com","action":{"forward":{"forward_url":"https://forward.example.com/in"}}}],"allow_ranges":[],"deny_ranges":[]}}"#,
    )
}

/// A `UserDefined` policy that only passes traffic through.
pub fn passthrough_policy() -> Result<NetworkPolicy, serde_json::Error> {
    serde_json::from_str(
        r#"{"user_defined":{"rules":[{"domain":"api.example.com"}],"allow_ranges":[],"deny_ranges":[]}}"#,
    )
}
