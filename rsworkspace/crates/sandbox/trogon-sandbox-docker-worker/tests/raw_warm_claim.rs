#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use futures::FutureExt;
use trogon_sandbox_contract::{CommandEnv, LeaseId, LeaseRequest, NativeHandleResponse, SandboxImage, StreamedOutput};
use trogon_sandbox_docker_worker::adapter::{self, DockerWorkerProvider};
use trogon_sandbox_docker_worker::digest_cache::DigestCache;
use trogon_sandbox_docker_worker::warm_pool::{WarmContainer, WarmPoolConfig};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_providers::LocalDockerProvider;

mod raw_warm_claim_support;
#[path = "targeted_process_support/snapshot.rs"]
mod snapshot;
mod targeted_process_support;

use raw_warm_claim_support::{FORBIDDEN_MARKER, KnownNative};
use targeted_process_support::{IMAGE, Native, Nats, fresh_id};

fn request(lease: &LeaseId, image: SandboxImage, vcpu_millis: Option<u32>) -> anyhow::Result<LeaseRequest> {
    targeted_process_support::lease_request("raw-warm-claim-tenant", lease, image, vcpu_millis)
}

async fn command(
    provider: &DockerWorkerProvider,
    lease: &LeaseId,
    script: String,
) -> anyhow::Result<trogon_sandbox_contract::CommandOutput> {
    provider
        .run_command_for_lease(
            lease,
            vec!["node".into(), "-e".into(), script],
            Some("/tmp".into()),
            &CommandEnv::default(),
            5,
            None,
            StreamedOutput::Disabled,
            4096,
        )
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_raw_warm_container_cannot_borrow_foreign_or_owner_label_lookalike_native_custody() -> anyhow::Result<()> {
    let nats = Nats::start().await?;
    let nonce = fresh_id();
    let owner = format!("raw-warm-provider-{nonce}");
    let fixture_owner = format!("raw-warm-fixture-{nonce}");
    let docker = bollard::Docker::connect_with_defaults()?.with_timeout(Duration::from_secs(20));
    let mut native = Native::new(owner.clone()).await?;
    let mut fixture_native = Native::new(fixture_owner.clone()).await?;
    let mut snapshot: Option<snapshot::SnapshotFixture> = None;
    let mut known: Vec<KnownNative> = vec![];
    let mut provider: Option<DockerWorkerProvider> = None;
    let observed = std::panic::AssertUnwindSafe(async {
        snapshot = Some(snapshot::SnapshotFixture::prepare(&docker, &mut fixture_native, &fixture_owner).await?);
        let snapshot = snapshot.as_mut().context("fixture state not prepared")?;
        provider = Some(DockerWorkerProvider::new(
            LocalDockerProvider::with_default_engine(IMAGE.into())?.with_owner(owner.clone()),
            IMAGE.into(), async_nats::jetstream::new(nats.client.clone()), nats.url.clone(),
            "raw-warm-library-worker".into(), adapter::build_oci_client(vec![trogon_sandbox_oci::RegistryHost::new(snapshot.reference.registry.clone())?]),
            snapshot.reference.registry.clone(), None, Arc::new(DigestCache::new(4)),
            trogon_sandbox_volumes::VolumeRegion::new("us-east-1").ok(), None,
            WarmPoolConfig { enabled: false, max_pool_size_per_digest: 1, max_total_warm: 1, max_idle_seconds: 600 },
        )?);
        let provider = provider.as_ref().context("provider not built")?;
        let image = SandboxImage::Snapshot { reference: snapshot.reference.clone(), parent: None };
        let positive_lease = LeaseId::new(format!("raw-warm-valid-positive-{nonce}"));
        native.register_lease(positive_lease.as_str());
        let positive = request(&positive_lease, image.clone(), Some(1000))?;
        let receipt = provider.lease(&positive, &positive_lease).await?;
        native.register_id(receipt.sandbox_id.as_str());
        native.running(receipt.sandbox_id.as_str(), positive_lease.as_str()).await?;
        let imported_id = snapshot.seal_import(&docker).await?;
        ensure!(docker.inspect_container(receipt.sandbox_id.as_str(), None).await?.image.as_ref() == Some(&imported_id),
            "raw claim positive did not use its actual immutable snapshot import");
        let output = command(provider, &positive_lease,
            format!("if(require('fs').readFileSync('/tmp/rollback-snapshot-control','utf8')!=={:?})process.exit(7);process.stdout.write('raw-claim-valid-snapshot')", snapshot.marker())).await?;
        ensure!(output.exit_code == 0 && output.stdout == b"raw-claim-valid-snapshot" && output.stderr.is_empty()
            && output.timed_out.is_within_deadline() && output.truncated.is_complete(), "raw claim valid snapshot ordinary command positive failed");
        provider.cancel(&positive_lease).await?;
        ensure!(native.absent(receipt.sandbox_id.as_str()).await? && provider.warm_pool_population().await.is_empty(),
            "sized positive must clean exactly and leave no implicit warm claim");
        eprintln!("raw warm claim positive: actual_valid_cold_snapshot=true ordinary_native_command=true canonical_own_native404=true owned_loopback_registry=true worker_protocol_scope=public_library_legacy signed_assignment_acceptance=false");

        let mut accepted = true;
        for (index, (case, native_owner)) in [("foreign_owner", &fixture_owner), ("owner_label_lookalike", &owner)].into_iter().enumerate() {
            let birth_lease = format!("raw-warm-{case}-{nonce}");
            let allocated = KnownNative::create(&docker, native_owner, &birth_lease, &imported_id).await?;
            let tracker = if native_owner == &owner { &mut native } else { &mut fixture_native };
            tracker.register_lease(&birth_lease);
            tracker.register_id(&allocated.id);
            known.push(allocated);
            let allocated = known.last().context("no known native")?;
            allocated.verify(&docker).await?;
            ensure!(allocated.forbidden_effect_absent(&docker).await?, "counterfeit command marker already exists before admission");
            let claim = WarmContainer::new(allocated.id.clone(), snapshot.reference.digest.clone(), imported_id.clone());
            ensure!(provider.warm_pool().lock().await.insert(claim).is_empty(), "raw fixture insertion evicted another native claim");
            ensure!(provider.warm_pool_population().await.get(&snapshot.reference.digest) == Some(&1), "public raw constructor claim was not inserted");
            let requested_lease = LeaseId::new(format!("raw-warm-request-{index}-{nonce}"));
            native.register_lease(requested_lease.as_str());
            let result = provider.lease(&request(&requested_lease, image.clone(), None)?, &requested_lease).await;
            let refused = result.is_err();
            let foreign_borrowed = result.as_ref().is_ok_and(|receipt| receipt.sandbox_id.as_str() == allocated.id);
            if let Ok(receipt) = &result { native.register_id(receipt.sandbox_id.as_str()); }
            let command_result = command(provider, &requested_lease,
                format!("require('fs').writeFileSync('{FORBIDDEN_MARKER}','untrusted-raw-claim');process.stdout.write('counterfeit-executed')")).await;
            let command_refused = command_result.is_err();
            let no_forbidden_effect = allocated.forbidden_effect_absent(&docker).await?;
            let native_birth_preserved = allocated.verify(&docker).await.is_ok();
            let handle_refused = matches!(provider.native_handle_for_lease(&requested_lease).await, NativeHandleResponse::Denied(_));
            eprintln!("raw warm claim primary receipt: case={case} actual_foreign_birth_positive=true valid_snapshot_control=true raw_constructor_only=true admission_refused={refused} foreign_cid_borrowed={foreign_borrowed} ordinary_command_refused={command_refused} forbidden_native_effect_absent={no_forbidden_effect} exact_original_native_birth_preserved={native_birth_preserved} public_handle_refused={handle_refused} signed_assignment_acceptance=false inventory_or_label_authority_claimed=false");
            accepted &= refused && !foreign_borrowed && command_refused && no_forbidden_effect && native_birth_preserved && handle_refused;
            if let Some(raw) = provider.warm_pool().lock().await.take(&snapshot.reference.digest) {
                ensure!(raw.container_id == allocated.id, "fixture refuses removing an unrelated pool claim");
            }
            allocated.finish(&docker).await?;
        }
        Ok::<_, anyhow::Error>(accepted)
    }).catch_unwind().await;

    let mut planned_cleanup = Ok(());
    for allocated in &known {
        if let Err(error) = allocated.finish(&docker).await {
            planned_cleanup = Err(error);
        }
    }
    let native_cleanup = native.finish().await;
    let fixture_cleanup = fixture_native.finish().await;
    let snapshot_cleanup = match &mut snapshot {
        Some(snapshot) => snapshot.finish(&docker).await,
        None => Ok(()),
    };
    planned_cleanup?;
    let product_fallback = native_cleanup?;
    let fixture_fallback = fixture_cleanup?;
    snapshot_cleanup?;
    eprintln!(
        "raw warm claim physical teardown: exact_planned_fixture_native404=true exact_imported_image_and_registry_cleanup=true product_fallback_used={product_fallback} fixture_fallback_used={fixture_fallback} provider_cancel_on_borrowed_foreign_never_called=true"
    );
    let accepted = match observed {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("the fixture body panicked"),
    };
    ensure!(
        accepted,
        "a public raw warm claim must not mint authority to borrow, execute inside or expose a foreign native"
    );
    ensure!(
        !product_fallback && !fixture_fallback,
        "physical fallback cannot promote raw warm claim acceptance"
    );
    Ok(())
}
