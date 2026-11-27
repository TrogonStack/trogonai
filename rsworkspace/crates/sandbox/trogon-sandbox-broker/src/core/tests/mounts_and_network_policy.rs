use super::*;

use trogon_sandbox_contract::{
    AllowRange, CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform,
    DomainPattern, DomainRule, DomainRuleAction, ForwardUrl, HeaderName, IpRange, IsolationTier, MatchRule,
    MountAccess, NetworkPolicy, PullSecretId, RemoteMount, RemoteMountSource, RequestForward, S3BucketName,
    SandboxSpecError, SetSnapshotTagRequest, SnapshotTagName, StringMatcher,
};

fn s3_mount(bucket: &str, path: &str, access: MountAccess) -> anyhow::Result<RemoteMount> {
    Ok(RemoteMount {
        source: RemoteMountSource::S3 {
            bucket: S3BucketName::new(bucket)?,
            prefix: String::new(),
        },
        mount_path: MountPath::new(path)?,
        access,
    })
}

/// A user-defined policy with only `rules`.
fn policy_with_rules(rules: Vec<DomainRule>) -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules,
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    }
}

/// A policy whose only rule matches paths with a regex that does not compile.
pub(super) fn bad_regex_policy() -> anyhow::Result<NetworkPolicy> {
    Ok(policy_with_rules(vec![DomainRule {
        domain: DomainPattern::new("example.com")?,
        action: DomainRuleAction::Transform(CredentialTransform {
            match_rules: vec![MatchRule {
                path: Some(StringMatcher::Regex("(unclosed".to_string())),
                methods: vec![],
                query_string: vec![],
                headers: vec![],
            }],
            credential: CredentialRequest {
                provider: CredentialProvider::Registry,
                pull_secret_id: PullSecretId::new("test-secret"),
                scope: CredentialScope::RegistryPull {
                    repository_glob: "*".to_string(),
                },
                ttl_seconds: 60,
                requester: None,
            },
            injection: CredentialInjection::Header(HeaderName::new("authorization")?),
        }),
    }]))
}

#[test]
fn a_relative_mount_path_cannot_be_constructed() {
    assert!(MountPath::new("relative/path").is_err());
}

#[test]
fn duplicate_mount_paths_are_rejected() -> TestResult {
    let mount = s3_mount("bkt", "/mnt/data", MountAccess::ReadWrite)?;
    let error = make_mount_request(vec![mount.clone(), mount])
        .err()
        .ok_or("duplicate mount paths must not build a request")?;
    assert!(
        matches!(
            error.downcast_ref::<SandboxSpecError>(),
            Some(SandboxSpecError::DuplicateMountPath(_))
        ),
        "expected DuplicateMountPath, got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn provider_without_fuse_capability_filtered_out_when_mounts_requested() -> TestResult {
    let mut no_fuse_profile = default_capability_profile();
    no_fuse_profile.provider = ProviderKind::K8sMicroVm;
    no_fuse_profile.volume.supports_fuse_mounts = CapabilitySupport::Unsupported;
    no_fuse_profile.isolation.default_tier = IsolationTier::MicroVm;

    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    registry
        .inject(worker_heartbeat("k8s-w1", ProviderKind::K8sMicroVm, no_fuse_profile)?)
        .await;

    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;
    let mounts = vec![s3_mount("my-bucket", "/mnt/data", MountAccess::ReadOnly)?];
    let mut req = edit_request_spec(make_request(vec![ProviderKind::K8sMicroVm])?, |parts| {
        parts.mounts = mounts;
    })?;
    req.idempotency_key = LeaseRequestKey::new("fuse-cap-test")?;

    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(
            response,
            LeaseResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { .. })
        ),
        "expected NoWorkerSatisfiesCapabilities, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn network_policy_allow_all_is_granted() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_network_policy_request(NetworkPolicy::AllowAll)?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    Ok(())
}

#[tokio::test]
async fn network_policy_deny_all_is_granted() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_network_policy_request(NetworkPolicy::DenyAll)?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    Ok(())
}

#[tokio::test]
async fn network_policy_user_defined_with_valid_fields_is_granted() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_network_policy_request(NetworkPolicy::UserDefined {
        rules: vec![
            DomainRule {
                domain: DomainPattern::new("example.com")?,
                action: DomainRuleAction::PassThrough,
            },
            DomainRule {
                domain: DomainPattern::new("*.internal.example.com")?,
                action: DomainRuleAction::Forward(RequestForward {
                    forward_url: ForwardUrl::new("https://proxy.example.com/relay")?,
                    match_rules: vec![],
                    ttl_seconds: RequestForward::DEFAULT_TTL_SECONDS,
                }),
            },
        ],
        allow_ranges: vec![AllowRange::new("10.0.0.0/8")?],
        deny_ranges: vec![IpRange::new("10.1.0.0/24")?],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })?;
    let response = core.handle_lease_request(req).await;
    let LeaseResponse::Granted(granted) = response else {
        return Err(format!("expected Granted, got {response:?}").into());
    };
    let record = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("active lease record should exist")?;
    assert!(
        matches!(record.spec.network_policy(), Some(NetworkPolicy::UserDefined { .. })),
        "granted lease's persisted spec should carry the validated network_policy"
    );
    Ok(())
}

#[test]
fn network_policy_bad_cidr_cannot_be_constructed() {
    assert!(IpRange::new("not-a-cidr").is_err());
}

#[tokio::test]
async fn network_policy_bad_regex_is_denied() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_network_policy_request(bad_regex_policy()?)?;
    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::InvalidNetworkPolicy { .. })),
        "expected InvalidNetworkPolicy, got {response:?}"
    );
    Ok(())
}

#[test]
fn network_policy_bad_wildcard_domain_cannot_be_constructed() {
    assert!(DomainPattern::new("foo.*.com").is_err());
    assert!(
        serde_json::from_str::<NetworkPolicy>(
            r#"{"user_defined":{"rules":[{"domain":"foo.*.com"}],"allow_ranges":[],"deny_ranges":[]}}"#
        )
        .is_err()
    );
}

#[test]
fn network_policy_forward_url_with_query_string_cannot_be_constructed() {
    assert!(ForwardUrl::new("https://proxy.example.com/relay?debug=1").is_err());
}

#[tokio::test]
async fn snapshot_tag_set_then_delete_roundtrip() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let tenant = TenantId::new("test-tenant")?;
    let digest = Digest::from_str(&format!("sha256:{}", "f".repeat(64)))?;

    let set_response = core
        .handle_snapshot_tag_set(SetSnapshotTagRequest {
            tenant: tenant.clone(),
            name: SnapshotTagName::new("latest")?,
            digest: digest.clone(),
        })
        .await;
    assert_eq!(set_response, SetSnapshotTagResponse::Applied);

    let delete_response = core
        .handle_snapshot_tag_delete(DeleteSnapshotTagRequest {
            tenant: tenant.clone(),
            name: SnapshotTagName::new("latest")?,
        })
        .await;
    assert_eq!(delete_response, DeleteSnapshotTagResponse::Applied);
    Ok(())
}
