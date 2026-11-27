use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use trogon_sandbox_contract::{
    ArtifactType, DomainRule, GetSandboxDomainResponse, LeaseError, LeaseId, LeasePersistence, LeasePrewarm,
    LeaseRequest, LeaseRequestKey, MountPath, NativeHandleResponse, NativeSnapshotId, NativeSnapshotRef, NetworkPolicy,
    OciImageRef, OpaqueTunnelPolicy, Port, PrivilegedAccess, ProviderKind, RemoteMount, RemoteMountSource,
    S3BucketName, SandboxFreezeResponse, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags,
    SnapshotExportOutcome, SnapshotTag, SnapshotTagName, TenantId, WorkerId, WorkspaceSource,
};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityResourceId, FreezeOutcome, HeldLease, Provider, ProviderError, PurgeOutcome, ThawOutcome,
};
use url::Url;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockGuard, MockServer, ResponseTemplate};

use super::{E2bWorkerProvider, NATIVE_OPTION_KEYS};
use crate::api::{E2bApi, E2bConfig};
use crate::envd::Envd;
use crate::exec::E2bExec;

pub(super) type TestResult = Result<(), Box<dyn std::error::Error>>;

const TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn build_provider(
    server: &MockServer,
    with_envd: bool,
) -> Result<E2bWorkerProvider, Box<dyn std::error::Error>> {
    let mut config = E2bConfig::new(Url::parse(&server.uri())?, "e2b.test", "k")?;
    if with_envd {
        config = config.with_envd_url(Url::parse(&server.uri())?)?;
    }
    let api = E2bApi::new(config, TIMEOUT)?;
    let exec = Arc::new(E2bExec::new(Envd::new(TIMEOUT)?));
    Ok(E2bWorkerProvider::new(api, "base", exec))
}

fn provider(server: &MockServer) -> Result<E2bWorkerProvider, Box<dyn std::error::Error>> {
    build_provider(server, false)
}

fn authority_provider(server: &MockServer) -> Result<E2bWorkerProvider, Box<dyn std::error::Error>> {
    build_provider(server, true)
}

fn spec_parts(image: SandboxImage) -> Result<SandboxSpecParts, Box<dyn std::error::Error>> {
    Ok(SandboxSpecParts {
        image,
        workspace: WorkspaceSource::empty(),
        env: Default::default(),
        cpu_seconds: std::num::NonZeroU32::MIN.saturating_add(3599),
        memory_mib: std::num::NonZeroU32::MIN.saturating_add(511),
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: vec![Port::new(3000)?],
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

fn registry(name: &str) -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(name),
        pull_secret: None,
    }
}

fn request_with(change: impl FnOnce(&mut SandboxSpecParts)) -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    let mut parts = spec_parts(registry("my-template"))?;
    change(&mut parts);
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("t")?,
        provider_pref: vec![ProviderKind::E2b],
        spec: SandboxSpec::new(parts)?,
        ttl_seconds: 600,
        idempotency_key: LeaseRequestKey::new("k")?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: LeasePrewarm::default(),
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::Persistent,
        native: None,
        region: None,
        fallback_regions: vec![],
    })
}

pub(super) fn request() -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    request_with(|_| {})
}

pub(super) async fn mount(server: &MockServer, verb: &str, route: &str, template: ResponseTemplate) -> MockGuard {
    server
        .register_as_scoped(Mock::given(method(verb)).and(path(route)).respond_with(template))
        .await
}

pub(super) async fn calls(guard: &MockGuard) -> usize {
    guard.received_requests().await.len()
}

pub(super) fn status(code: u16, body: Value) -> ResponseTemplate {
    ResponseTemplate::new(code).set_body_json(body)
}

pub(super) async fn created(server: &MockServer) -> MockGuard {
    mount(
        server,
        "POST",
        "/v2/sandboxes",
        status(
            201,
            json!({
                "sandboxID": "sbx1",
                "templateID": "my-template",
                "envdAccessToken": "envd-tok",
                "trafficAccessToken": "traffic-tok",
                "domain": null
            }),
        ),
    )
    .await
}

fn held_sbx1(id: &LeaseId) -> Result<HeldLease, Box<dyn std::error::Error>> {
    Ok(HeldLease::Provider {
        lease_id: id.clone(),
        provider: ProviderKind::E2b,
        resource_id: AuthorityResourceId::new("sbx1")?,
    })
}

#[tokio::test]
async fn a_lease_creates_a_sandbox_from_the_named_template() -> TestResult {
    let server = MockServer::start().await;
    let create = server
        .register_as_scoped(
            Mock::given(method("POST"))
                .and(path("/v2/sandboxes"))
                .and(body_partial_json(json!({"templateID": "my-template", "timeout": 600})))
                .respond_with(status(201, json!({"sandboxID": "sbx1", "templateID": "my-template"}))),
        )
        .await;
    let p = provider(&server)?;
    let lease = p.lease(&request()?, &LeaseId::new("l1")).await?;
    assert_eq!(lease.sandbox_id.as_str(), "sbx1");
    assert_eq!(calls(&create).await, 1);
    assert_eq!(p.active_lease_count().await, 1);
    let sandbox = p.sandbox_for_lease(&LeaseId::new("l1")).await.ok_or("lease not held")?;
    assert_eq!(sandbox.domain, "e2b.test");
    assert!(p.sandbox_for_lease(&LeaseId::new("other")).await.is_none());
    assert_eq!(p.kind(), ProviderKind::E2b);
    Ok(())
}

#[tokio::test]
async fn the_default_template_fills_an_empty_repository() -> TestResult {
    let server = MockServer::start().await;
    let p = provider(&server)?;
    assert_eq!(p.template_for(&registry(""))?, "base");
    Ok(())
}

#[tokio::test]
async fn an_empty_default_template_is_refused() -> TestResult {
    let server = MockServer::start().await;
    let config = E2bConfig::new(Url::parse(&server.uri())?, "e2b.test", "k")?;
    let exec = Arc::new(E2bExec::new(Envd::new(TIMEOUT)?));
    let p = E2bWorkerProvider::new(E2bApi::new(config, TIMEOUT)?, "", exec);
    assert!(matches!(
        p.template_for(&registry("")),
        Err(super::E2bProviderError::TemplateRequired)
    ));
    Ok(())
}

#[tokio::test]
async fn env_tags_network_and_auto_pause_reach_the_create_body() -> TestResult {
    let server = MockServer::start().await;
    let create = server
        .register_as_scoped(
            Mock::given(method("POST"))
                .and(path("/v2/sandboxes"))
                .and(body_partial_json(json!({
                    "envVars": {"A": "1"},
                    "metadata": {"team": "x"},
                    "autoPause": true,
                    "allow_internet_access": false
                })))
                .respond_with(status(201, json!({"sandboxID": "sbx1"}))),
        )
        .await;
    let p = provider(&server)?;
    let mut req = request_with(|parts| {
        parts.env.insert("A".into(), "1".into());
        parts.network_policy = Some(NetworkPolicy::DenyAll);
    })?;
    req.tags = SandboxTags::new([("team".to_owned(), "x".to_owned())].into())?;
    req.native = Some(json!({"e2b": {"auto_pause": true}}));
    p.lease(&req, &LeaseId::new("l1")).await?;
    assert_eq!(calls(&create).await, 1);
    assert_eq!(NATIVE_OPTION_KEYS, ["auto_pause"]);
    Ok(())
}

#[tokio::test]
async fn refusals_happen_before_any_create_call() -> TestResult {
    let server = MockServer::start().await;
    let create = created(&server).await;
    let p = provider(&server)?;

    let mut unknown = request()?;
    unknown.native = Some(json!({"e2b": {"typo": 1}}));
    assert!(p.lease(&unknown, &LeaseId::new("a")).await.is_err());

    let mount_spec = RemoteMount {
        source: RemoteMountSource::S3 {
            bucket: S3BucketName::new("bucket")?,
            prefix: "p".into(),
        },
        mount_path: MountPath::new("/mnt")?,
        access: Default::default(),
    };
    let mounts = request_with(|parts| parts.mounts = vec![mount_spec])?;
    assert!(p.lease(&mounts, &LeaseId::new("m")).await.is_err());

    let net = request_with(|parts| {
        parts.network_policy = Some(NetworkPolicy::UserDefined {
            rules: Vec::<DomainRule>::new(),
            allow_ranges: vec![],
            deny_ranges: vec![],
            denied_domains: vec![],
            opaque_tunnels: OpaqueTunnelPolicy::Refuse,
            request_rules: vec![],
        });
    })?;
    assert!(p.lease(&net, &LeaseId::new("b")).await.is_err());

    let tag = SnapshotTag {
        tenant: TenantId::new("t")?,
        name: SnapshotTagName::new("prod")?,
    };
    let snap = request_with(|parts| {
        parts.image = SandboxImage::SnapshotTag {
            tag,
            artifact_type: ArtifactType::SandboxRootfsV1,
            create_from: None,
        };
    })?;
    assert!(p.lease(&snap, &LeaseId::new("c")).await.is_err());
    assert_eq!(calls(&create).await, 0);
    assert_eq!(p.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_second_lease_under_the_same_id_is_refused() -> TestResult {
    let server = MockServer::start().await;
    let create = created(&server).await;
    let p = provider(&server)?;
    let id = LeaseId::new("dup");
    p.lease(&request()?, &id).await?;
    assert!(p.lease(&request()?, &id).await.is_err());
    assert_eq!(calls(&create).await, 1);
    Ok(())
}

#[tokio::test]
async fn a_vendor_failure_surfaces_and_registers_nothing() -> TestResult {
    let server = MockServer::start().await;
    let _create = mount(
        &server,
        "POST",
        "/v2/sandboxes",
        status(429, json!({"code": 429, "message": "slow down"})).insert_header("Retry-After", "7"),
    )
    .await;
    let p = provider(&server)?;
    let error = p.lease(&request()?, &LeaseId::new("l1")).await;
    assert!(error.is_err());
    assert_eq!(p.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn cancel_deletes_the_sandbox_and_is_idempotent() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let delete = mount(&server, "DELETE", "/sandboxes/sbx1", ResponseTemplate::new(204)).await;
    let absent = mount(
        &server,
        "GET",
        "/sandboxes/sbx1",
        status(404, json!({"message": "not found"})),
    )
    .await;
    let p = provider(&server)?;
    let id = LeaseId::new("l1");
    p.lease(&request()?, &id).await?;
    p.cancel(&id).await?;
    p.cancel(&id).await?;
    assert_eq!(calls(&delete).await, 1);
    assert_eq!(calls(&absent).await, 1);
    assert_eq!(p.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn failed_cancel_retains_exact_resource_until_cleanup_is_confirmed() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let unavailable = mount(
        &server,
        "DELETE",
        "/sandboxes/sbx1",
        status(503, json!({"message": "retry cleanup"})),
    )
    .await;
    let p = authority_provider(&server)?;
    let lease_id = LeaseId::new("cleanup-retry");
    assert_eq!(p.lease(&request()?, &lease_id).await?.sandbox_id.as_str(), "sbx1");
    assert!(
        p.cancel(&lease_id).await.is_err(),
        "failed backend cleanup remains an error"
    );
    assert_eq!(calls(&unavailable).await, 1);
    drop(unavailable);
    let removed = mount(&server, "DELETE", "/sandboxes/sbx1", ResponseTemplate::new(204)).await;
    let absent = mount(
        &server,
        "GET",
        "/sandboxes/sbx1",
        status(404, json!({"message": "not found"})),
    )
    .await;
    p.cancel(&lease_id).await?;
    assert_eq!(calls(&removed).await, 1);
    assert_eq!(calls(&absent).await, 1);
    assert_eq!(p.active_lease_count().await, 0);
    p.cancel(&lease_id).await?;
    assert_eq!(calls(&removed).await, 1);
    Ok(())
}

#[tokio::test]
async fn authority_keeps_exact_native_inventory_until_positive_cleanup() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let p = authority_provider(&server)?;
    let worker = WorkerId::new("e2b-owner")?;
    p.bind_worker(&worker)?;
    p.bind_worker(&worker)?;
    assert!(p.bind_worker(&WorkerId::new("other-owner")?).is_err());
    let id = LeaseId::new("authority-held");
    assert_eq!(p.lease(&request()?, &id).await?.worker_id, Some(worker));
    let held = p.held_leases().await?;
    assert_eq!(held, vec![held_sbx1(&id)?]);
    let alien = HeldLease::Provider {
        lease_id: id.clone(),
        provider: ProviderKind::E2b,
        resource_id: AuthorityResourceId::new("other-vm")?,
    };
    let _alien_state = mount(
        &server,
        "GET",
        "/sandboxes/other-vm",
        status(200, json!({"state": "running"})),
    )
    .await;
    let foreign_delete = mount(&server, "DELETE", "/sandboxes/other-vm", ResponseTemplate::new(204)).await;
    assert_eq!(p.freeze(&alien).await?, FreezeOutcome::Stale);
    assert_eq!(p.thaw(&alien).await?, ThawOutcome::Stale);
    assert_eq!(AuthorityHost::purge(&p, &alien).await?, PurgeOutcome::Stale);
    let other_provider = HeldLease::Provider {
        lease_id: id.clone(),
        provider: ProviderKind::Daytona,
        resource_id: AuthorityResourceId::new("sbx1")?,
    };
    assert_eq!(p.freeze(&other_provider).await?, FreezeOutcome::Stale);
    assert_eq!(calls(&foreign_delete).await, 0);
    assert_eq!(p.held_leases().await?, held);

    let unavailable = mount(&server, "DELETE", "/sandboxes/sbx1", ResponseTemplate::new(503)).await;
    assert!(AuthorityHost::purge(&p, &held[0]).await.is_err());
    assert_eq!(p.held_leases().await?, held);
    drop(unavailable);
    let removed = mount(&server, "DELETE", "/sandboxes/sbx1", ResponseTemplate::new(204)).await;
    assert_eq!(AuthorityHost::purge(&p, &held[0]).await?, PurgeOutcome::Purged);
    assert!(p.held_leases().await?.is_empty());
    assert_eq!(calls(&removed).await, 1);
    let present = mount(
        &server,
        "GET",
        "/sandboxes/sbx1",
        status(200, json!({"state": "running"})),
    )
    .await;
    assert_eq!(
        AuthorityHost::purge(&p, &held[0]).await?,
        PurgeOutcome::Stale,
        "map absence cannot confirm cleanup"
    );
    drop(present);
    let _gone = mount(&server, "GET", "/sandboxes/sbx1", ResponseTemplate::new(404)).await;
    assert_eq!(AuthorityHost::purge(&p, &held[0]).await?, PurgeOutcome::Settled);
    assert_eq!(calls(&removed).await, 1);
    Ok(())
}

#[tokio::test]
async fn authority_memory_pause_retains_failed_thaw_and_yields_to_manual_pause() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("authority-pause");
    p.lease(&request()?, &id).await?;
    let held = p.held_leases().await?.remove(0);
    let _state = mount(
        &server,
        "GET",
        "/sandboxes/sbx1",
        status(200, json!({"state": "running"})),
    )
    .await;
    let pause = mount(&server, "POST", "/sandboxes/sbx1/pause", ResponseTemplate::new(204)).await;
    assert_eq!(p.freeze(&held).await?, FreezeOutcome::Frozen);
    let unavailable = mount(
        &server,
        "POST",
        "/v2/sandboxes/sbx1/connect",
        ResponseTemplate::new(503),
    )
    .await;
    assert!(matches!(
        p.snapshot_lease(&id, "must-stay-frozen").await,
        SnapshotExportOutcome::Err(_)
    ));
    assert_eq!(calls(&unavailable).await, 0);
    assert!(p.thaw(&held).await.is_err());
    assert_eq!(p.held_leases().await?, vec![held.clone()]);
    assert_eq!(calls(&unavailable).await, 1);
    drop(unavailable);
    let resumed = mount(
        &server,
        "POST",
        "/v2/sandboxes/sbx1/connect",
        status(200, json!({"sandboxID": "sbx1", "envdAccessToken": "fresh-token"})),
    )
    .await;
    assert_eq!(p.thaw(&held).await?, ThawOutcome::Thawed);
    assert_eq!(calls(&resumed).await, 1);
    assert_eq!(p.freeze(&held).await?, FreezeOutcome::Frozen);
    assert!(matches!(
        p.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(p.thaw(&held).await?, ThawOutcome::NoLongerOwned);
    assert_eq!(calls(&resumed).await, 1);
    assert_eq!(calls(&pause).await, 3);
    Ok(())
}

#[tokio::test]
async fn authority_does_not_claim_or_thaw_an_externally_paused_vm() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("external-pause");
    p.lease(&request()?, &id).await?;
    let held = p.held_leases().await?.remove(0);
    let _state = mount(
        &server,
        "GET",
        "/sandboxes/sbx1",
        status(200, json!({"state": "paused"})),
    )
    .await;
    let pause = mount(&server, "POST", "/sandboxes/sbx1/pause", ResponseTemplate::new(204)).await;
    let resume = mount(
        &server,
        "POST",
        "/v2/sandboxes/sbx1/connect",
        ResponseTemplate::new(200),
    )
    .await;
    assert_eq!(p.freeze(&held).await?, FreezeOutcome::AlreadyPaused);
    assert_eq!(p.thaw(&held).await?, ThawOutcome::NoLongerOwned);
    assert_eq!(calls(&pause).await, 0);
    assert_eq!(calls(&resume).await, 0);
    Ok(())
}

#[tokio::test]
async fn freeze_refuses_unconfirmed_states_and_settles_pause_conflicts() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("freeze-edges");
    p.lease(&request()?, &id).await?;
    let held = p.held_leases().await?.remove(0);

    let missing = mount(&server, "GET", "/sandboxes/sbx1", ResponseTemplate::new(404)).await;
    assert_eq!(p.freeze(&held).await?, FreezeOutcome::Stale);
    drop(missing);

    let odd = mount(
        &server,
        "GET",
        "/sandboxes/sbx1",
        status(200, json!({"state": "snapshotting"})),
    )
    .await;
    assert!(p.freeze(&held).await.is_err());
    drop(odd);

    let _running = mount(
        &server,
        "GET",
        "/sandboxes/sbx1",
        status(200, json!({"state": "running"})),
    )
    .await;
    let conflict = mount(
        &server,
        "POST",
        "/sandboxes/sbx1/pause",
        status(409, json!({"code": 409, "message": "already paused"})),
    )
    .await;
    assert!(p.freeze(&held).await.is_err());
    drop(conflict);

    let rejected = mount(
        &server,
        "POST",
        "/sandboxes/sbx1/pause",
        status(400, json!({"code": 400, "message": "no"})),
    )
    .await;
    assert!(p.freeze(&held).await.is_err());
    drop(rejected);
    Ok(())
}

#[tokio::test]
async fn freeze_after_conflict_confirms_an_already_paused_sandbox() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("freeze-conflict");
    p.lease(&request()?, &id).await?;
    let held = p.held_leases().await?.remove(0);
    let running = mount(
        &server,
        "GET",
        "/sandboxes/sbx1",
        status(200, json!({"state": "running"})),
    )
    .await;
    let _conflict = mount(
        &server,
        "POST",
        "/sandboxes/sbx1/pause",
        status(409, json!({"code": 409, "message": "already paused"})),
    )
    .await;
    assert!(p.freeze(&held).await.is_err());
    drop(running);
    // A manual pause is respected: the first read is running, the confirmation read is paused.
    let first = server
        .register_as_scoped(
            Mock::given(method("GET"))
                .and(path("/sandboxes/sbx1"))
                .respond_with(status(200, json!({"state": "running"})))
                .up_to_n_times(1),
        )
        .await;
    let _then_paused = mount(
        &server,
        "GET",
        "/sandboxes/sbx1",
        status(200, json!({"state": "paused"})),
    )
    .await;
    assert_eq!(p.freeze(&held).await?, FreezeOutcome::AlreadyPaused);
    drop(first);
    Ok(())
}

#[tokio::test]
async fn a_cleanup_only_lease_refuses_freeze_and_thaw_reports_stale() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let _unavailable = mount(&server, "DELETE", "/sandboxes/sbx1", ResponseTemplate::new(503)).await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("cleanup-only");
    p.lease(&request()?, &id).await?;
    let held = p.held_leases().await?.remove(0);
    assert!(p.cancel(&id).await.is_err());
    assert!(p.freeze(&held).await.is_err());
    let _present = mount(
        &server,
        "GET",
        "/sandboxes/sbx1",
        status(200, json!({"state": "running"})),
    )
    .await;
    assert_eq!(p.thaw(&held).await?, ThawOutcome::Stale);
    assert!(p.sandbox_for_lease(&id).await.is_none());
    assert!(matches!(
        p.native_handle_for_lease(&id).await,
        NativeHandleResponse::Denied(_)
    ));
    assert!(matches!(
        p.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn pause_and_resume_map_to_the_native_verbs() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let pause = mount(&server, "POST", "/sandboxes/sbx1/pause", ResponseTemplate::new(204)).await;
    let connect = mount(
        &server,
        "POST",
        "/v2/sandboxes/sbx1/connect",
        status(200, json!({"sandboxID": "sbx1", "envdAccessToken": "new-tok"})),
    )
    .await;
    let p = provider(&server)?;
    let id = LeaseId::new("l1");
    p.lease(&request()?, &id).await?;
    assert!(matches!(
        p.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(calls(&pause).await, 1);
    assert!(matches!(
        p.set_paused_for_lease(&id, false).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(calls(&connect).await, 1);
    assert!(matches!(
        p.set_paused_for_lease(&LeaseId::new("nope"), true).await,
        SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn pausing_twice_is_idempotent_and_failures_are_denied() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let conflict = mount(
        &server,
        "POST",
        "/sandboxes/sbx1/pause",
        status(409, json!({"code": 409, "message": "already paused"})),
    )
    .await;
    let p = provider(&server)?;
    let id = LeaseId::new("l1");
    p.lease(&request()?, &id).await?;
    assert!(matches!(
        p.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Done
    ));
    drop(conflict);
    let _boom = mount(
        &server,
        "POST",
        "/sandboxes/sbx1/pause",
        status(500, json!({"code": 500, "message": "boom"})),
    )
    .await;
    assert!(matches!(
        p.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Denied(LeaseError::Backend { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn the_native_handle_names_the_sandbox() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let p = provider(&server)?;
    let id = LeaseId::new("l1");
    p.lease(&request()?, &id).await?;
    let NativeHandleResponse::Available(handle) = p.native_handle_for_lease(&id).await else {
        return Err("expected a native handle".into());
    };
    assert_eq!(handle.provider, ProviderKind::E2b);
    assert_eq!(handle.vendor_id, "sbx1");
    assert!(matches!(
        p.native_handle_for_lease(&LeaseId::new("x")).await,
        NativeHandleResponse::Denied(_)
    ));
    Ok(())
}

async fn preview_admission_create(server: &MockServer, sandbox_id: &str, traffic_token: &str) -> MockGuard {
    mount(
        server,
        "POST",
        "/v2/sandboxes",
        status(
            201,
            json!({
                "sandboxID": sandbox_id, "templateID": "my-template",
                "envdAccessToken": "preview-envd-fixture", "trafficAccessToken": traffic_token,
            }),
        ),
    )
    .await
}

async fn cancel_confirmed(server: &MockServer, p: &E2bWorkerProvider, id: &LeaseId, sandbox: &str) -> TestResult {
    let _delete = mount(
        server,
        "DELETE",
        &format!("/sandboxes/{sandbox}"),
        ResponseTemplate::new(204),
    )
    .await;
    let _absent = mount(
        server,
        "GET",
        &format!("/sandboxes/{sandbox}"),
        ResponseTemplate::new(404),
    )
    .await;
    p.cancel(id).await?;
    assert_eq!(p.active_lease_count().await, 0);
    Ok(())
}

async fn preview_admission_replacement_must_be_refused(replacement_id: &str) -> TestResult {
    let server = MockServer::start().await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("preview-admission-replacement");
    let first = preview_admission_create(&server, "preview-first", "preview-first-fixture").await;
    p.lease(&request()?, &id).await?;
    let admitted = p.sandbox_for_lease(&id).await.ok_or("lease not held")?;
    assert_eq!(admitted.sandbox_id, "preview-first");
    assert_eq!(calls(&first).await, 1);
    drop(first);
    cancel_confirmed(&server, &p, &id, "preview-first").await?;

    let _replacement = preview_admission_create(&server, replacement_id, "preview-replacement-fixture").await;
    p.lease(&request()?, &id).await?;
    let current = p.sandbox_for_lease(&id).await.ok_or("lease not held")?;
    assert_eq!(current.sandbox_id, replacement_id);
    assert_ne!(admitted.traffic_token, current.traffic_token);

    let stale_response = p.get_domain_for_lease(&id, &admitted, 3000).await;
    let current_response = p.get_domain_for_lease(&id, &current, 3000).await;
    let current_usable = matches!(
        current_response,
        GetSandboxDomainResponse::Available { endpoint, access: Some(access), .. }
            if endpoint == format!("3000-{replacement_id}.e2b.test:443")
                && access.base_url == format!("https://3000-{replacement_id}.e2b.test")
                && access.headers.get("e2b-traffic-access-token")
                    .is_some_and(|value| value == "preview-replacement-fixture")
    );
    cancel_confirmed(&server, &p, &id, replacement_id).await?;
    assert!(current_usable, "replacement admission must remain usable");
    assert!(
        matches!(
            stale_response,
            GetSandboxDomainResponse::Denied(LeaseError::LeaseNotFound { lease_id }) if lease_id == id
        ),
        "the earlier preview admission must not disclose a replacement native resource or credential"
    );
    Ok(())
}

#[tokio::test]
async fn preview_admission_refuses_different_native_replacement() -> TestResult {
    preview_admission_replacement_must_be_refused("preview-replacement").await
}

#[tokio::test]
async fn preview_admission_refuses_same_native_id_with_fresh_credential_instance() -> TestResult {
    preview_admission_replacement_must_be_refused("preview-first").await
}

#[tokio::test]
async fn preview_admission_accepts_current_credential_after_same_instance_reconnect() -> TestResult {
    let server = MockServer::start().await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("preview-admission-refresh");
    let create = preview_admission_create(&server, "preview-refresh", "preview-original-fixture").await;
    p.lease(&request()?, &id).await?;
    let admitted = p.sandbox_for_lease(&id).await.ok_or("lease not held")?;
    let connect = mount(
        &server,
        "POST",
        "/v2/sandboxes/preview-refresh/connect",
        status(
            201,
            json!({
                "sandboxID": "preview-refresh", "templateID": "my-template",
                "envdAccessToken": "preview-refreshed-envd-fixture",
                "trafficAccessToken": "preview-refreshed-fixture",
            }),
        ),
    )
    .await;
    assert!(matches!(
        p.set_paused_for_lease(&id, false).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(calls(&create).await, 1);
    assert_eq!(calls(&connect).await, 1);
    let current = p.sandbox_for_lease(&id).await.ok_or("lease not held")?;
    assert_eq!(current.sandbox_id, admitted.sandbox_id);
    assert_ne!(admitted.traffic_token, current.traffic_token);
    let response = p.get_domain_for_lease(&id, &admitted, 3000).await;
    cancel_confirmed(&server, &p, &id, "preview-refresh").await?;
    assert!(
        matches!(
            response,
            GetSandboxDomainResponse::Available { endpoint, access: Some(access), .. }
                if endpoint == "3000-preview-refresh.e2b.test:443"
                    && access.base_url == "https://3000-preview-refresh.e2b.test"
                    && access.headers.get("e2b-traffic-access-token")
                        .is_some_and(|value| value == "preview-refreshed-fixture")
        ),
        "same-instance reconnect must use the current shared credential despite the stale admission snapshot"
    );
    Ok(())
}

#[tokio::test]
async fn snapshot_of_an_unknown_or_badly_named_lease_is_refused() -> TestResult {
    let server = MockServer::start().await;
    let p = provider(&server)?;
    assert!(matches!(
        p.snapshot_lease(&LeaseId::new("none"), "fine-name").await,
        SnapshotExportOutcome::Err(_)
    ));
    assert!(matches!(
        p.snapshot_lease(&LeaseId::new("none"), "").await,
        SnapshotExportOutcome::Err(_)
    ));
    Ok(())
}

fn foreign_held(id: &LeaseId) -> Result<HeldLease, Box<dyn std::error::Error>> {
    Ok(HeldLease::Provider {
        lease_id: id.clone(),
        provider: ProviderKind::Subprocess,
        resource_id: AuthorityResourceId::new("sbx1")?,
    })
}

async fn running_state(server: &MockServer) -> MockGuard {
    mount(
        server,
        "GET",
        "/sandboxes/sbx1",
        status(200, json!({"state": "running"})),
    )
    .await
}

#[tokio::test]
async fn a_privileged_lease_is_refused_as_unsupported() -> TestResult {
    let server = MockServer::start().await;
    let create = created(&server).await;
    let p = provider(&server)?;
    let privileged = request_with(|parts| parts.privileged = PrivilegedAccess::Granted)?;
    assert!(matches!(
        p.lease(&privileged, &LeaseId::new("p")).await,
        Err(ProviderError::PrivilegedUnsupported(_))
    ));
    assert_eq!(calls(&create).await, 0);
    Ok(())
}

#[tokio::test]
async fn a_second_lookup_of_the_same_snapshot_shares_its_gate() -> TestResult {
    let server = MockServer::start().await;
    let p = provider(&server)?;
    let tenant = TenantId::new("t")?;
    let reference = NativeSnapshotRef {
        provider: ProviderKind::E2b,
        id: NativeSnapshotId::new("snap")?,
        expires_at: None,
    };
    let first = p.snapshot_gate(&tenant, &reference);
    let second = p.snapshot_gate(&tenant, &reference);
    assert!(Arc::ptr_eq(&first, &second));
    Ok(())
}

#[tokio::test]
async fn resume_failures_are_denied() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let _pause = mount(&server, "POST", "/sandboxes/sbx1/pause", ResponseTemplate::new(204)).await;
    let _connect = mount(
        &server,
        "POST",
        "/v2/sandboxes/sbx1/connect",
        status(500, json!({"code": 500, "message": "boom"})),
    )
    .await;
    let p = provider(&server)?;
    let id = LeaseId::new("l1");
    p.lease(&request()?, &id).await?;
    assert!(matches!(
        p.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Done
    ));
    assert!(matches!(
        p.set_paused_for_lease(&id, false).await,
        SandboxFreezeResponse::Denied(LeaseError::Backend { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_sandbox_id_the_protocol_cannot_name_is_deleted_and_refused() -> TestResult {
    let server = MockServer::start().await;
    let _create = mount(
        &server,
        "POST",
        "/v2/sandboxes",
        status(201, json!({"sandboxID": "bad\nid", "templateID": "my-template"})),
    )
    .await;
    let p = provider(&server)?;
    assert!(p.lease(&request()?, &LeaseId::new("l1")).await.is_err());
    assert_eq!(p.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_domain_the_worker_cannot_address_is_deleted_and_refused() -> TestResult {
    let server = MockServer::start().await;
    let _create = mount(
        &server,
        "POST",
        "/v2/sandboxes",
        status(
            201,
            json!({"sandboxID": "sbx1", "templateID": "my-template", "domain": "bad domain/x"}),
        ),
    )
    .await;
    let delete = mount(&server, "DELETE", "/sandboxes/sbx1", ResponseTemplate::new(204)).await;
    let p = provider(&server)?;
    assert!(p.lease(&request()?, &LeaseId::new("l1")).await.is_err());
    assert_eq!(calls(&delete).await, 1);
    assert_eq!(p.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn freeze_surfaces_a_failed_state_lookup() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let _state = mount(&server, "GET", "/sandboxes/sbx1", ResponseTemplate::new(500)).await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("l1");
    p.lease(&request()?, &id).await?;
    assert!(p.freeze(&held_sbx1(&id)?).await.is_err());
    Ok(())
}

#[tokio::test]
async fn freezing_a_manually_paused_lease_never_claims_it() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let _pause = mount(&server, "POST", "/sandboxes/sbx1/pause", ResponseTemplate::new(204)).await;
    let _state = running_state(&server).await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("l1");
    p.lease(&request()?, &id).await?;
    assert!(matches!(
        p.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(p.freeze(&held_sbx1(&id)?).await?, FreezeOutcome::AlreadyPaused);
    Ok(())
}

#[tokio::test]
async fn a_pause_conflict_on_a_manual_lease_stays_unconfirmed_while_running() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let _pause = mount(
        &server,
        "POST",
        "/sandboxes/sbx1/pause",
        status(409, json!({"code": 409, "message": "busy"})),
    )
    .await;
    let _state = running_state(&server).await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("l1");
    p.lease(&request()?, &id).await?;
    assert!(matches!(
        p.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Done
    ));
    assert!(p.freeze(&held_sbx1(&id)?).await.is_err());
    Ok(())
}

#[tokio::test]
async fn foreign_provider_resources_are_stale_for_every_authority_verb() -> TestResult {
    let server = MockServer::start().await;
    let p = authority_provider(&server)?;
    let held = foreign_held(&LeaseId::new("l1"))?;
    assert_eq!(p.freeze(&held).await?, FreezeOutcome::Stale);
    assert_eq!(p.thaw(&held).await?, ThawOutcome::Stale);
    assert_eq!(AuthorityHost::purge(&p, &held).await?, PurgeOutcome::Stale);
    Ok(())
}

#[tokio::test]
async fn an_unheld_lease_is_settled_by_the_vendor_state() -> TestResult {
    let server = MockServer::start().await;
    let p = authority_provider(&server)?;
    let held = held_sbx1(&LeaseId::new("gone"))?;
    let missing = mount(&server, "GET", "/sandboxes/sbx1", ResponseTemplate::new(404)).await;
    assert_eq!(p.thaw(&held).await?, ThawOutcome::NoLongerOwned);
    assert_eq!(AuthorityHost::purge(&p, &held).await?, PurgeOutcome::Settled);
    drop(missing);
    let _broken = mount(&server, "GET", "/sandboxes/sbx1", ResponseTemplate::new(500)).await;
    assert!(p.thaw(&held).await.is_err());
    assert!(AuthorityHost::purge(&p, &held).await.is_err());
    Ok(())
}

#[tokio::test]
async fn thaw_forgets_a_sandbox_that_vanished_while_frozen() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let _state = running_state(&server).await;
    let _pause = mount(&server, "POST", "/sandboxes/sbx1/pause", ResponseTemplate::new(204)).await;
    let _connect = mount(
        &server,
        "POST",
        "/v2/sandboxes/sbx1/connect",
        status(404, json!({"code": 404, "message": "gone"})),
    )
    .await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("l1");
    p.lease(&request()?, &id).await?;
    let held = held_sbx1(&id)?;
    assert_eq!(p.freeze(&held).await?, FreezeOutcome::Frozen);
    assert_eq!(p.thaw(&held).await?, ThawOutcome::NoLongerOwned);
    assert_eq!(p.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn purge_settles_when_the_vendor_no_longer_has_the_sandbox() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let _delete = mount(&server, "DELETE", "/sandboxes/sbx1", ResponseTemplate::new(404)).await;
    let p = authority_provider(&server)?;
    let id = LeaseId::new("l1");
    p.lease(&request()?, &id).await?;
    assert_eq!(AuthorityHost::purge(&p, &held_sbx1(&id)?).await?, PurgeOutcome::Settled);
    assert_eq!(p.active_lease_count().await, 0);
    Ok(())
}
