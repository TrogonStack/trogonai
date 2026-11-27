use async_nats::jetstream;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_admin::{
    AdminError, Console, api_key, constants, prewarm, quota, snapshot, tag, tenant_policy, worker,
};
use trogon_sandbox_auth::{ApiKeyBucket, NatsKvApiKeyStore};
use trogon_sandbox_broker::host_mount_policy::NatsKvHostMountAllowList;
use trogon_sandbox_broker::image_policy::NatsKvImageAllowList;
use trogon_sandbox_broker::limits::NatsKvTenantLimits;
use trogon_sandbox_broker::native_policy::NatsKvNativePassthroughStore;
use trogon_sandbox_broker::quota::NatsKvQuotaStore;
use trogon_sandbox_broker::worker_fence::WorkerPlacementState;
use trogon_sandbox_contract::{
    ArtifactType, Digest, LeaseId, ProviderKind, ServiceAccountId, SnapshotTag, SnapshotTagName, TenantId, WorkerId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Buffers {
    out: Vec<u8>,
    err: Vec<u8>,
}

impl Buffers {
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.out).into_owned()
    }

    fn err(&self) -> String {
        String::from_utf8_lossy(&self.err).into_owned()
    }
}

fn tenant(raw: &str) -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(raw.parse()?)
}

fn digest() -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(format!("sha256:{}", "ab".repeat(32)))?)
}

async fn server() -> (JetStreamTestServer, jetstream::Context) {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    (server, js)
}

#[tokio::test]
async fn tags_round_trip_through_set_get_list_history_and_clear() -> TestResult {
    let (_server, js) = server().await;
    let acme = tenant("acme")?;
    let tag_ref = SnapshotTag {
        tenant: acme.clone(),
        name: "stable".parse::<SnapshotTagName>()?,
    };

    let mut buffers = Buffers::default();
    {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        tag::set(
            js.clone(),
            tag_ref.clone(),
            digest()?,
            ArtifactType::HyperlightSnapshotV1,
            None,
            &mut console,
        )
        .await?;
        tag::get(js.clone(), &tag_ref, &mut console).await?;
        tag::list(js.clone(), &acme, &mut console).await?;
        tag::history(js.clone(), &acme, &tag_ref.name, &mut console).await?;
    }
    assert!(buffers.out().contains("stable"));
    assert!(buffers.out().contains(digest()?.as_str()));

    let mut cleared = Buffers::default();
    {
        let mut console = Console::new(&mut cleared.out, &mut cleared.err);
        tag::clear(js.clone(), &tag_ref, &mut console).await?;
    }
    assert!(cleared.err().contains("cleared tag"));

    let mut after = Buffers::default();
    let mut console = Console::new(&mut after.out, &mut after.err);
    let missing = tag::get(js, &tag_ref, &mut console).await;
    assert!(matches!(missing, Err(AdminError::TagNotFound { .. })));
    Ok(())
}

#[tokio::test]
async fn tag_set_with_a_stale_revision_conflicts() -> TestResult {
    let (_server, js) = server().await;
    let tag_ref = SnapshotTag {
        tenant: tenant("acme")?,
        name: "stable".parse::<SnapshotTagName>()?,
    };
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    tag::set(
        js.clone(),
        tag_ref.clone(),
        digest()?,
        ArtifactType::SbomV1,
        None,
        &mut console,
    )
    .await?;
    let stale = tag::set(js, tag_ref, digest()?, ArtifactType::SbomV1, Some(9_999), &mut console).await;
    assert!(stale.is_err());
    Ok(())
}

#[tokio::test]
async fn prewarm_policies_set_show_and_clear() -> TestResult {
    let (_server, js) = server().await;
    let acme = tenant("acme")?;
    let mut buffers = Buffers::default();
    {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        prewarm::show(js.clone(), &acme, &mut console).await?;
        prewarm::set(
            js.clone(),
            &acme,
            &digest()?,
            2,
            vec![ProviderKind::LocalDocker],
            &mut console,
        )
        .await?;
        prewarm::show(js.clone(), &acme, &mut console).await?;
        prewarm::clear(js.clone(), &acme, &digest()?, &mut console).await?;
        prewarm::show(js, &acme, &mut console).await?;
    }
    let out = buffers.out();
    assert_eq!(out.matches("no prewarm policies").count(), 2, "{out}");
    assert!(out.contains(digest()?.as_str()), "{out}");
    Ok(())
}

#[tokio::test]
async fn image_policy_merges_replaces_and_clears() -> TestResult {
    let (_server, js) = server().await;
    let acme = tenant("acme")?;
    let store = NatsKvImageAllowList::new(js, constants::IMAGE_ALLOWLIST_BUCKET).await?;
    let mut buffers = Buffers::default();
    {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        tenant_policy::image_policy_set(&store, &acme, vec!["a/*".into()], false, &mut console).await?;
        tenant_policy::image_policy_set(&store, &acme, vec!["b".into()], false, &mut console).await?;
        tenant_policy::image_policy_show(&store, &acme, &mut console).await?;
    }
    let merged = buffers.out();
    assert!(merged.contains("a/*") && merged.contains("\"b\""), "{merged}");

    let mut replaced = Buffers::default();
    {
        let mut console = Console::new(&mut replaced.out, &mut replaced.err);
        tenant_policy::image_policy_set(&store, &acme, vec!["c".into()], true, &mut console).await?;
        tenant_policy::image_policy_show(&store, &acme, &mut console).await?;
        tenant_policy::image_policy_clear(&store, &acme, &mut console).await?;
    }
    let out = replaced.out();
    assert!(out.contains("\"c\"") && !out.contains("a/*"), "{out}");
    Ok(())
}

#[tokio::test]
async fn host_mount_native_passthrough_and_limits_round_trip() -> TestResult {
    let (_server, js) = server().await;
    let acme = tenant("acme")?;
    let mounts = NatsKvHostMountAllowList::new(js.clone(), constants::HOST_MOUNT_ALLOWLIST_BUCKET).await?;
    let native = NatsKvNativePassthroughStore::new(js.clone(), constants::NATIVE_PASSTHROUGH_BUCKET).await?;
    let limits = NatsKvTenantLimits::new(js, constants::TENANT_LIMITS_BUCKET).await?;

    let mut buffers = Buffers::default();
    {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        tenant_policy::host_mount_set(&mounts, &acme, vec!["/srv/work".into()], false, &mut console).await?;
        tenant_policy::host_mount_show(&mounts, &acme, &mut console).await?;
        tenant_policy::native_passthrough_set(&native, &acme, true, &mut console).await?;
        tenant_policy::native_passthrough_show(&native, &acme, &mut console).await?;
        tenant_policy::tenant_limit_set(&limits, &acme, 10, 512, None, &mut console).await?;
        tenant_policy::tenant_limit_show(&limits, &acme, &mut console).await?;
    }
    let out = buffers.out();
    assert!(out.contains("/srv/work"), "{out}");
    assert!(out.contains("512"), "{out}");

    let mut cleared = Buffers::default();
    let mut console = Console::new(&mut cleared.out, &mut cleared.err);
    tenant_policy::host_mount_clear(&mounts, &acme, &mut console).await?;
    tenant_policy::native_passthrough_clear(&native, &acme, &mut console).await?;
    tenant_policy::tenant_limit_clear(&limits, &acme, &mut console).await?;
    Ok(())
}

#[tokio::test]
async fn api_keys_mint_list_and_revoke_by_token() -> TestResult {
    let (_server, js) = server().await;
    let bucket = ApiKeyBucket::new("admin-test-keys")?;
    let store = NatsKvApiKeyStore::new_with_bucket(js, &bucket).await?;
    let acme = tenant("acme")?;

    let mut minted = Buffers::default();
    {
        let mut console = Console::new(&mut minted.out, &mut minted.err);
        api_key::mint(&store, acme.clone(), ServiceAccountId::new("sa"), "ci", &mut console).await?;
    }
    let token = minted.out().trim().to_string();
    assert!(api_key::is_raw_token(&token), "{token}");
    assert!(minted.err().contains("store this token securely"));

    let mut listed = Buffers::default();
    {
        let mut console = Console::new(&mut listed.out, &mut listed.err);
        api_key::list(&store, Some(&acme), &mut console).await?;
    }
    assert!(listed.out().contains("ci"), "{}", listed.out());
    assert!(!listed.out().contains(&token));

    let mut revoked = Buffers::default();
    {
        let mut console = Console::new(&mut revoked.out, &mut revoked.err);
        api_key::revoke(&store, &token, &mut console).await?;
    }

    let mut after = Buffers::default();
    let mut console = Console::new(&mut after.out, &mut after.err);
    api_key::list(&store, Some(&acme), &mut console).await?;
    assert!(!after.out().contains("ci"), "{}", after.out());
    Ok(())
}

#[tokio::test]
async fn worker_fence_cordon_status_and_list() -> TestResult {
    let (_server, js) = server().await;
    let worker_id: WorkerId = "w1".parse()?;
    let mut buffers = Buffers::default();
    {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        worker::list(js.clone(), &mut console).await?;
        worker::set_state(
            js.clone(),
            constants::DEFAULT_ACTIVE_LEASE_BUCKET,
            ProviderKind::LocalDocker,
            &worker_id,
            WorkerPlacementState::Cordoned,
            Some("kernel patch".into()),
            &mut console,
        )
        .await?;
        worker::status(
            js.clone(),
            constants::DEFAULT_ACTIVE_LEASE_BUCKET,
            ProviderKind::LocalDocker,
            &worker_id,
            &mut console,
        )
        .await?;
        worker::list(js, &mut console).await?;
    }
    let out = buffers.out();
    assert!(out.contains("no fenced workers"), "{out}");
    assert!(out.contains("w1"), "{out}");
    Ok(())
}

#[tokio::test]
async fn quota_show_reports_an_empty_month() -> TestResult {
    let (_server, js) = server().await;
    let store = NatsKvQuotaStore::new(js, constants::QUOTA_BUCKET).await?;
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    quota::show(&store, &tenant("acme")?, Some((2024, 3)), &mut console).await?;
    assert!(!buffers.out().is_empty() || !buffers.err().is_empty());
    Ok(())
}

#[tokio::test]
async fn snapshot_status_without_a_capture_is_an_error() -> TestResult {
    let (_server, js) = server().await;
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    let result = snapshot::status(js, &LeaseId::new("lease-1"), &mut console).await;
    assert!(matches!(result, Err(AdminError::NoCaptureStatus { .. })));
    Ok(())
}
