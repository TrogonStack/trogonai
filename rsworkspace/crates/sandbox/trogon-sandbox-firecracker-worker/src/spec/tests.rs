use std::num::NonZeroU32;

use anyhow::{Context, bail};
use trogon_sandbox_contract::{
    CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform, DomainPattern,
    DomainRule, DriveId, DriveMount, DriveName, EnvVarName, ForwardUrl, FsPathEntry, FsPolicy, HeaderName,
    HostMountAccess, InjectHosts, LeasePersistence, LeasePrewarm, LeaseRequestKey, LiteralPath, MountPath, OciImageRef,
    ProviderKind, PullSecretId, RemoteMount, RemoteMountSource, RequestForward, S3BucketName, SandboxSpec,
    SandboxSpecParts, SandboxTags, SecretBinding, SecretBindingParts, SecretBindings, SecretMode, SecretName,
    SecretTarget, TenantId,
};

use super::*;
use crate::config::{
    BaseRootfs, EgressProxyBinary, FirecrackerBinary, KernelImage, NetnsHelper, NetnsPrefix, NetworkConfig,
    RelayBinary, SnapshotConfig, SnapshotDir, StateDir,
};

const IMAGE: &str = "registry.example/guest:1";

fn config() -> FirecrackerConfig {
    let mut config = FirecrackerConfig::new(
        FirecrackerBinary::new("firecracker"),
        KernelImage::new("/k"),
        BaseRootfs::new("/r"),
        StateDir::new("/s"),
    );
    config.default_image = OciImageRef::parse_unqualified(IMAGE);
    config
}

fn registry_image(reference: &str) -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(reference),
        pull_secret: None,
    }
}

fn spec_parts() -> anyhow::Result<SandboxSpecParts> {
    Ok(SandboxSpecParts {
        image: registry_image(IMAGE),
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(1).context("zero cpu seconds")?,
        memory_mib: NonZeroU32::new(256).context("zero memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })
}

fn request(edit: impl FnOnce(&mut SandboxSpecParts) -> anyhow::Result<()>) -> anyhow::Result<LeaseRequest> {
    let mut parts = spec_parts()?;
    edit(&mut parts)?;
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("t")?,
        provider_pref: vec![ProviderKind::Firecracker],
        spec: SandboxSpec::new(parts)?,
        ttl_seconds: 60,
        idempotency_key: LeaseRequestKey::new("k")?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: LeasePrewarm::from(false),
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::from(false),
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}

fn refusal(config: &FirecrackerConfig, request: &LeaseRequest) -> anyhow::Result<ProviderError> {
    match accept(config, request) {
        Err(error) => Ok(error),
        Ok(accepted) => bail!("the request was accepted: {accepted:?}"),
    }
}

fn rejected(edit: impl FnOnce(&mut SandboxSpecParts) -> anyhow::Result<()>, needle: &str) -> anyhow::Result<()> {
    let error = refusal(&config(), &request(edit)?)?;
    assert!(matches!(error, ProviderError::InvalidSpec(_)), "{error}");
    assert!(format!("{error:?}").contains(needle), "{error}");
    Ok(())
}

fn allow_list(action: DomainRuleAction) -> anyhow::Result<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new("example.com")?,
            action,
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn credential_request(glob: &str) -> CredentialRequest {
    CredentialRequest {
        provider: CredentialProvider::Registry,
        pull_secret_id: PullSecretId::new("injected"),
        scope: CredentialScope::RegistryPull {
            repository_glob: glob.into(),
        },
        ttl_seconds: 60,
        requester: None,
    }
}

fn transform_policy() -> anyhow::Result<NetworkPolicy> {
    allow_list(DomainRuleAction::Transform(CredentialTransform {
        match_rules: vec![],
        credential: credential_request("*"),
        injection: CredentialInjection::Header(HeaderName::new("Authorization")?),
    }))
}

fn forward_policy() -> anyhow::Result<NetworkPolicy> {
    allow_list(DomainRuleAction::Forward(RequestForward {
        forward_url: ForwardUrl::new("https://upstream.example")?,
        match_rules: vec![],
        ttl_seconds: 60,
    }))
}

fn host_links_config(broker: bool) -> anyhow::Result<FirecrackerConfig> {
    let mut config = config();
    config.network = Some(crate::network::NetworkConfig {
        host_links: "10.231.0.0/24".parse()?,
    });
    config.credential_broker_nats_url = broker.then(|| "nats://broker".to_string());
    Ok(config)
}

fn networked_config() -> anyhow::Result<FirecrackerConfig> {
    let mut config = config();
    config.legacy_network = Some(NetworkConfig {
        helper: NetnsHelper::new("/usr/local/sbin/fc-netns"),
        proxy_binary: EgressProxyBinary::new("sandbox-egress-proxyd"),
        relay_binary: RelayBinary::new("trogon-sandbox-firecracker-worker"),
        prefix: NetnsPrefix::new("sbxfc-")?,
    });
    Ok(config)
}

fn digest() -> anyhow::Result<Digest> {
    Ok(Digest::new(format!("sha256:{}", "a".repeat(64)))?)
}

fn snapshot(artifact_type: ArtifactType) -> anyhow::Result<SandboxImage> {
    Ok(SandboxImage::Snapshot {
        reference: OciArtifactRef {
            registry: "snapshots.local".into(),
            repository: "t".into(),
            digest: digest()?,
            artifact_type,
        },
        parent: None,
    })
}

fn snapshotting_config() -> anyhow::Result<FirecrackerConfig> {
    let mut config = networked_config()?;
    config.snapshots = Some(SnapshotConfig::new(SnapshotDir::new("/snapshots")));
    Ok(config)
}

#[test]
fn a_plain_lease_is_accepted_with_its_own_memory() -> anyhow::Result<()> {
    let accepted = accept(&config(), &request(|_| Ok(()))?)?;
    assert_eq!(accepted.memory.get(), 256);
    assert_eq!(accepted.working_dir, DEFAULT_WORKING_DIR);
    Ok(())
}

#[test]
fn zero_memory_never_reaches_the_worker_as_a_lease() -> anyhow::Result<()> {
    let mut wire = serde_json::to_value(request(|_| Ok(()))?)?;
    let spec = wire
        .get_mut("spec")
        .and_then(serde_json::Value::as_object_mut)
        .context("a lease request carries its spec")?;
    spec.insert("memory_mib".to_string(), serde_json::json!(0));
    assert!(serde_json::from_value::<LeaseRequest>(wire).is_err());
    Ok(())
}

#[test]
fn an_oversized_env_is_refused() -> anyhow::Result<()> {
    let refused = request(|parts| {
        for i in 0..=SandboxSpec::MAX_ENV_ENTRIES {
            parts.env.insert(format!("K{i}"), "v".to_string());
        }
        Ok(())
    });
    let error = match refused {
        Ok(request) => bail!("the oversized env was accepted: {:?}", request.spec.env().len()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("env"), "{error}");
    Ok(())
}

#[test]
fn an_explicit_deny_all_policy_is_the_only_policy_accepted() -> anyhow::Result<()> {
    accept(
        &config(),
        &request(|p| {
            p.network_policy = Some(NetworkPolicy::DenyAll);
            Ok(())
        })?,
    )?;
    rejected(
        |p| {
            p.network_policy = Some(NetworkPolicy::AllowAll);
            Ok(())
        },
        "allow_all",
    )
}

#[test]
fn other_images_are_refused() -> anyhow::Result<()> {
    rejected(
        |p| {
            p.image = registry_image("other:1");
            Ok(())
        },
        "cannot pull images",
    )
}

#[test]
fn mounts_drives_and_ports_are_refused() -> anyhow::Result<()> {
    rejected(
        |p| {
            p.mounts = vec![RemoteMount {
                source: RemoteMountSource::S3 {
                    bucket: S3BucketName::new("bucket")?,
                    prefix: "p".into(),
                },
                mount_path: MountPath::new("/mnt")?,
                access: Default::default(),
            }];
            Ok(())
        },
        "remote mounts",
    )?;
    rejected(
        |p| {
            p.drives = vec![DriveMount {
                drive_id: DriveId {
                    tenant: TenantId::new("t")?,
                    name: DriveName::new("d")?,
                },
                mount_path: MountPath::new("/d")?,
                mode: Default::default(),
            }];
            Ok(())
        },
        "persistent drives",
    )?;
    rejected(
        |p| {
            p.exposed_ports = vec![trogon_sandbox_contract::Port::new(8080)?];
            Ok(())
        },
        "exposed_ports",
    )
}

#[test]
fn workspaces_that_need_a_network_or_the_host_are_refused() -> anyhow::Result<()> {
    rejected(
        |p| {
            p.workspace = WorkspaceSource::HostMount {
                path: "/h".into(),
                access: HostMountAccess::ReadOnly,
            };
            Ok(())
        },
        "host_mount",
    )
}

#[test]
fn credential_injection_needs_a_broker() -> anyhow::Result<()> {
    let policy = transform_policy()?;
    let req = request(|p| {
        p.network_policy = Some(policy);
        Ok(())
    })?;
    for config in [networked_config()?, host_links_config(false)?] {
        let error = refusal(&config, &req)?;
        assert!(matches!(error, ProviderError::InvalidSpec(_)), "{error}");
    }
    let error = refusal(&host_links_config(false)?, &req)?;
    assert!(format!("{error:?}").contains("credential injection"), "{error}");
    accept(&host_links_config(true)?, &req)?;
    Ok(())
}

#[test]
fn the_legacy_network_injects_credentials_only_with_a_broker() -> anyhow::Result<()> {
    let policy = transform_policy()?;
    let req = request(|p| {
        p.network_policy = Some(policy);
        Ok(())
    })?;
    let mut legacy = networked_config()?;
    legacy.credential_broker_nats_url = Some("nats://broker".to_string());
    accept(&legacy, &req)?;
    assert!(legacy.injects_credentials() && legacy.intercepts_tls());
    // Forward rules still need host-links, even with a signing key.
    legacy.oidc_signing_key_pem_file = Some("/keys/oidc.pem".into());
    assert!(!legacy.forwards_requests());
    let forward_policy = forward_policy()?;
    let forward = request(|p| {
        p.network_policy = Some(forward_policy);
        Ok(())
    })?;
    refusal(&legacy, &forward)?;
    let error = refusal(&networked_config()?, &req)?;
    assert!(format!("{error:?}").contains("--credential-broker-nats-url"), "{error}");
    Ok(())
}

#[test]
fn host_links_without_a_broker_still_accepts_leases_that_inject_nothing() -> anyhow::Result<()> {
    let policy = allow_list(DomainRuleAction::PassThrough)?;
    let plain = request(|p| {
        p.network_policy = Some(policy);
        Ok(())
    })?;
    accept(&host_links_config(false)?, &plain)?;
    Ok(())
}

#[test]
fn request_forwarding_needs_host_links_and_a_signing_key() -> anyhow::Result<()> {
    let policy = forward_policy()?;
    let req = request(|p| {
        p.network_policy = Some(policy);
        Ok(())
    })?;
    let error = refusal(&host_links_config(true)?, &req)?;
    assert!(format!("{error:?}").contains("forwarding"), "{error}");

    let mut with_key = host_links_config(false)?;
    with_key.oidc_signing_key_pem_file = Some("/keys/oidc.pem".into());
    accept(&with_key, &req)?;
    Ok(())
}

#[test]
fn a_signing_key_does_not_let_the_legacy_network_forward_requests() -> anyhow::Result<()> {
    let mut legacy = networked_config()?;
    legacy.oidc_signing_key_pem_file = Some("/keys/oidc.pem".into());
    let policy = forward_policy()?;
    let req = request(|p| {
        p.network_policy = Some(policy);
        Ok(())
    })?;
    refusal(&legacy, &req)?;
    Ok(())
}

#[test]
fn deny_all_and_no_policy_boot_without_a_network_device() -> anyhow::Result<()> {
    for policy in [None, Some(NetworkPolicy::DenyAll)] {
        let chosen = policy.clone();
        let accepted = accept(
            &networked_config()?,
            &request(|p| {
                p.network_policy = chosen;
                Ok(())
            })?,
        )?;
        assert_eq!(accepted.network_policy, None, "{policy:?}");
    }
    Ok(())
}

#[test]
fn egress_policies_need_a_worker_with_network_devices() -> anyhow::Result<()> {
    rejected(
        |p| {
            p.network_policy = Some(NetworkPolicy::AllowAll);
            Ok(())
        },
        "--netns-helper",
    )?;
    let policy = allow_list(DomainRuleAction::PassThrough)?;
    let chosen = policy.clone();
    let accepted = accept(
        &networked_config()?,
        &request(|p| {
            p.network_policy = Some(chosen);
            Ok(())
        })?,
    )?;
    assert_eq!(accepted.network_policy, Some(policy));
    Ok(())
}

#[test]
fn secret_binding_security_requirements_are_not_ignored() -> anyhow::Result<()> {
    let policy = allow_list(DomainRuleAction::PassThrough)?;
    let binding = SecretBinding::new(SecretBindingParts {
        name: SecretName::new("registry-token")?,
        target: SecretTarget::Env(EnvVarName::new("REGISTRY_TOKEN")?),
        mode: SecretMode::Mask,
        extract: None,
        mask_duplicates: Default::default(),
        on_extract_no_match: Default::default(),
        strip_response_headers: Default::default(),
        inject_hosts: InjectHosts::new(vec![DomainPattern::new("example.com")?])?,
        inject_scope: Default::default(),
        source: credential_request("example/*"),
    })?;
    let req = request(|p| {
        p.network_policy = Some(policy);
        p.secret_bindings = SecretBindings::new(vec![binding])?;
        Ok(())
    })?;
    let error = refusal(&networked_config()?, &req)?;
    assert!(matches!(error, ProviderError::InvalidSpec(_)), "{error}");
    assert!(format!("{error:?}").contains("secret_bindings"), "{error}");
    Ok(())
}

#[test]
fn filesystem_security_requirements_are_not_ignored() -> anyhow::Result<()> {
    let entry = FsPathEntry::Literal {
        path: LiteralPath::new("/private")?,
        directory: Default::default(),
    };
    let req = request(|p| {
        p.fs_policy = Some(FsPolicy::new(vec![entry], vec![], vec![], vec![], vec![], vec![])?);
        Ok(())
    })?;
    let error = refusal(&config(), &req)?;
    assert!(matches!(error, ProviderError::InvalidSpec(_)), "{error}");
    assert!(format!("{error:?}").contains("fs_policy"), "{error}");
    let empty = request(|p| {
        p.fs_policy = Some(FsPolicy::default());
        Ok(())
    })?;
    accept(&config(), &empty)?;
    Ok(())
}

#[test]
fn rules_that_need_tls_interception_are_refused() -> anyhow::Result<()> {
    let policy = forward_policy()?;
    let error = refusal(
        &networked_config()?,
        &request(|p| {
            p.network_policy = Some(policy);
            Ok(())
        })?,
    )?;
    assert!(format!("{error:?}").contains("TLS interception"), "{error}");
    Ok(())
}

#[test]
fn a_firecracker_snapshot_can_use_portable_storage_or_a_local_snapshot_store() -> anyhow::Result<()> {
    let image = snapshot(ArtifactType::FirecrackerSnapshotV1)?;
    let chosen = image.clone();
    let portable = accept(
        &config(),
        &request(|p| {
            p.image = chosen;
            Ok(())
        })?,
    )?;
    assert_eq!(portable.origin, Origin::Snapshot(digest()?));
    let accepted = accept(
        &snapshotting_config()?,
        &request(|p| {
            p.image = image;
            Ok(())
        })?,
    )?;
    assert_eq!(accepted.origin, Origin::Snapshot(digest()?));
    Ok(())
}

#[test]
fn another_provider_s_snapshot_is_refused() -> anyhow::Result<()> {
    let image = snapshot(ArtifactType::HyperlightSnapshotV1)?;
    let error = refusal(
        &snapshotting_config()?,
        &request(|p| {
            p.image = image;
            Ok(())
        })?,
    )?;
    assert!(format!("{error:?}").contains("only its own snapshots"), "{error}");
    Ok(())
}

#[test]
fn a_restore_cannot_gain_a_network_device() -> anyhow::Result<()> {
    let image = snapshot(ArtifactType::FirecrackerSnapshotV1)?;
    let policy = allow_list(DomainRuleAction::PassThrough)?;
    let (restored, networked) = (image.clone(), policy);
    let error = refusal(
        &snapshotting_config()?,
        &request(|p| {
            p.image = restored;
            p.network_policy = Some(networked);
            Ok(())
        })?,
    )?;
    assert!(format!("{error:?}").contains("restored microVM"), "{error}");
    accept(
        &snapshotting_config()?,
        &request(|p| {
            p.image = image;
            p.network_policy = Some(NetworkPolicy::DenyAll);
            Ok(())
        })?,
    )?;
    Ok(())
}
