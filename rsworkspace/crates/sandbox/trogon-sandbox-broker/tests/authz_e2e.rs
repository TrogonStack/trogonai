//! Runs the broker binary against a private NATS server with authz enforced and
//! checks that one tenant's API key cannot reach another tenant's lease or
//! account. A cross-tenant request is refused on the key's tenant before the
//! authorizer is asked, so a TCP listener stands in for SpiceDB.

use std::collections::BTreeMap;
use std::error::Error;
use std::num::NonZeroU32;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime};

use async_nats::HeaderMap;
use async_nats::jetstream;
use futures::StreamExt;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_auth::{ApiKeyBucket, NatsKvApiKeyStore};
use trogon_sandbox_broker::active_lease::{ActiveLeaseStore, NatsKvActiveLeaseStore};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, ExtendLeaseTimeoutRequest, ExtendLeaseTimeoutResponse,
    GetTenantUsageRequest, GetTenantUsageResponse, LaunchKey, LeaseError, LeaseId, LeaseRecord, LeaseStatus,
    OciImageRef, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, ServiceAccountId, TenantId,
    WorkspaceSource,
};
use trogon_sandbox_protocol::subjects::{TENANT_USAGE_REQUEST, lease_cancel_command_launch, lease_extend_timeout};

type TestResult = Result<(), Box<dyn Error>>;

const API_KEY_BUCKET: &str = "sandbox-authz-explicit-api-keys";
const ACTIVE_LEASE_BUCKET: &str = "sandbox-active-leases-authz-e2e";
const ANSWER_ATTEMPTS: usize = 60;

/// Kills the child process when the test ends, however it ends.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn bearer(key: &str) -> Result<HeaderMap, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert("Authorization", format!("Bearer {key}").as_str());
    Ok(headers)
}

fn spec() -> Result<SandboxSpec, Box<dyn Error>> {
    Ok(SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("ubuntu:24.04"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(60).ok_or("zero cpu")?,
        memory_mib: NonZeroU32::new(512).ok_or("zero memory")?,
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
    })?)
}

fn lease_of(tenant: &TenantId, lease_id: &LeaseId) -> Result<LeaseRecord, Box<dyn Error>> {
    Ok(LeaseRecord {
        epoch: None,
        entity_id: None,
        lease_id: lease_id.clone(),
        tenant: tenant.clone(),
        provider: ProviderKind::Mock,
        status: LeaseStatus::Running,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(300),
        spec: spec()?,
        sandbox_id: "sandbox-b".to_string(),
        cpu_seconds_consumed: 0,
        wall_seconds_consumed: 0,
        correlation_id: None,
        tags: SandboxTags::default(),
        name: None,
        worker_id: None,
        dispatch_attempt: None,
        quota_reservation: None,
        last_activity_at: None,
        paused_remaining_seconds: None,
        freeze_in_flight: None,
        auto_paused_at: None,
        idle_pause_retry: None,
        dormancy: None,
        restored_from: None,
    })
}

fn unauthorized_reason(error: &LeaseError) -> Option<&str> {
    match error {
        LeaseError::Unauthorized { reason } => Some(reason),
        _ => None,
    }
}

/// A TCP listener that accepts connections and never answers, standing in for SpiceDB.
async fn silent_spicedb() -> Result<std::net::SocketAddr, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    Ok(address)
}

/// Sends a request until the broker, still starting, answers it.
async fn request_with_retry(
    client: &async_nats::Client,
    subject: String,
    headers: HeaderMap,
    payload: Vec<u8>,
) -> Result<bytes::Bytes, Box<dyn Error>> {
    for _ in 0..ANSWER_ATTEMPTS {
        let request = client.request_with_headers(subject.clone(), headers.clone(), payload.clone().into());
        if let Ok(Ok(message)) = tokio::time::timeout(Duration::from_secs(1), request).await {
            return Ok(message.payload);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(format!("the broker never answered {subject}").into())
}

#[tokio::test]
async fn a_key_for_one_tenant_is_refused_on_another_tenants_lease_and_account() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let spicedb = silent_spicedb().await?;

    let tenant_a = TenantId::new("tenant-a")?;
    let tenant_b = TenantId::new("tenant-b")?;
    let keys = NatsKvApiKeyStore::new_with_bucket(js.clone(), &ApiKeyBucket::new(API_KEY_BUCKET)?).await?;
    let key_a = keys.mint(tenant_a.clone(), ServiceAccountId::new("sa-a"), "a").await?;
    let key_a = key_a.expose_secret().to_string();

    let lease_b = LeaseId::new("lease-of-tenant-b");
    NatsKvActiveLeaseStore::new(js.clone(), ACTIVE_LEASE_BUCKET)
        .await?
        .put(&lease_of(&tenant_b, &lease_b)?)
        .await?;

    let _broker = Reaped(
        Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-broker"))
            .env("NATS_URL", &url)
            .env("SANDBOX_API_KEY_BUCKET", API_KEY_BUCKET)
            .env("SANDBOX_AUTHZ_ENFORCE", "true")
            .env("SPICEDB_ENDPOINT", format!("http://{spicedb}"))
            .env("SPICEDB_TOKEN", "unused")
            .env("SANDBOX_ACTIVE_LEASE_BUCKET", ACTIVE_LEASE_BUCKET)
            .env("SANDBOX_JETSTREAM_REPLICAS", "1")
            .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );

    // Tenant B's lease, asked for with tenant A's key.
    let extend = serde_json::to_vec(&ExtendLeaseTimeoutRequest {
        lease_id: lease_b.clone(),
        additional_seconds: 60,
    })?;
    let reply = request_with_retry(&client, lease_extend_timeout(&lease_b), bearer(&key_a)?, extend.clone()).await?;
    let ExtendLeaseTimeoutResponse::Denied(error) = serde_json::from_slice(&reply)? else {
        return Err("tenant A extended tenant B's lease".into());
    };
    assert_eq!(unauthorized_reason(&error), Some("TenantMismatch"), "{error:?}");

    // Tenant B's account, asked for with tenant A's key.
    let usage = serde_json::to_vec(&GetTenantUsageRequest {
        tenant: tenant_b.clone(),
    })?;
    let reply = request_with_retry(&client, TENANT_USAGE_REQUEST.to_string(), bearer(&key_a)?, usage).await?;
    let GetTenantUsageResponse::Denied(error) = serde_json::from_slice(&reply)? else {
        return Err("tenant A read tenant B's usage".into());
    };
    assert_eq!(unauthorized_reason(&error), Some("TenantMismatch"), "{error:?}");

    // The refusal is about the key, not the request: no key at all reads differently.
    let reply = request_with_retry(&client, lease_extend_timeout(&lease_b), HeaderMap::new(), extend).await?;
    let ExtendLeaseTimeoutResponse::Denied(error) = serde_json::from_slice(&reply)? else {
        return Err("an unauthenticated caller extended tenant B's lease".into());
    };
    assert_eq!(unauthorized_reason(&error), Some("MissingHeader"), "{error:?}");

    let mut worker_cancels = client.subscribe("sandbox.provider.*.cancel_command_launch.*").await?;
    client.flush().await?;
    let cancel = serde_json::to_vec(&CancelCommandLaunchRequest {
        lease_id: lease_b.clone(),
        launch_key: LaunchKey::new("owned-launch")?,
    })?;
    for (headers, expected_reason) in [
        (bearer(&key_a)?, "TenantMismatch"),
        (HeaderMap::new(), "MissingHeader"),
        (bearer("unknown-cancellation-key")?, "UnknownKey"),
    ] {
        let reply = request_with_retry(&client, lease_cancel_command_launch(&lease_b), headers, cancel.clone()).await?;
        let CancelCommandLaunchResponse::Denied(error) = serde_json::from_slice(&reply)? else {
            return Err("a refused caller reached command launch cancellation".into());
        };
        assert_eq!(unauthorized_reason(&error), Some(expected_reason), "{error:?}");
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), worker_cancels.next())
            .await
            .is_err(),
        "refused cancellation callers must never reach a provider"
    );
    Ok(())
}

#[tokio::test]
async fn the_default_configuration_refuses_an_unauthenticated_request() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    let client = server.client().await;
    let spicedb = silent_spicedb().await?;

    // No SANDBOX_AUTHZ_ENFORCE and no --insecure-dev: enforcement comes from the defaults.
    let _broker = Reaped(
        Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-broker"))
            .env_remove("SANDBOX_AUTHZ_ENFORCE")
            .env_remove("SANDBOX_INSECURE_DEV")
            .env("NATS_URL", &url)
            .env("SPICEDB_ENDPOINT", format!("http://{spicedb}"))
            .env("SPICEDB_TOKEN", "unused")
            .env("SANDBOX_JETSTREAM_REPLICAS", "1")
            .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );

    let usage = serde_json::to_vec(&GetTenantUsageRequest {
        tenant: TenantId::new("any-tenant")?,
    })?;
    let reply = request_with_retry(&client, TENANT_USAGE_REQUEST.to_string(), HeaderMap::new(), usage).await?;
    let GetTenantUsageResponse::Denied(error) = serde_json::from_slice(&reply)? else {
        return Err("a default broker served an unauthenticated request".into());
    };
    assert_eq!(unauthorized_reason(&error), Some("MissingHeader"), "{error:?}");
    Ok(())
}
