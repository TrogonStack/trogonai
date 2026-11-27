mod support;

use std::time::Duration;

use serde_json::json;
use support::lease::{Lease, clone_of, host_mount, no_auth, snapshot_image, volume};
use support::provider::{Fixture, config};
use support::{TestResult, fake_aca::FakeAca};
use trogon_sandbox_aca_worker::{AcaError, SessionDeleteOutcome, SessionStatus};
use trogon_sandbox_contract::{CredentialProvider, CredentialScope, LeaseId, LifecycleEvent, ProviderKind, WorkerId};
use trogon_sandbox_protocol::{Provider, ProviderError};

fn lease_id(value: &str) -> LeaseId {
    LeaseId::new(value)
}

fn aca_error(error: ProviderError) -> Result<AcaError, Box<dyn std::error::Error>> {
    match error {
        ProviderError::Backend(source) => Ok(*source
            .downcast::<AcaError>()
            .map_err(|_| "the backend error is not an ACA error")?),
        other => Err(other.into()),
    }
}

fn git(repo: &std::path::Path, args: &[&str]) -> TestResult {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=tester",
            "-c",
            "user.email=tester@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {args:?} failed").into())
    }
}

async fn failure(fixture: &Fixture, lease: Lease, id: &str) -> Result<AcaError, Box<dyn std::error::Error>> {
    let error = fixture
        .provider
        .lease(&lease.build()?, &lease_id(id))
        .await
        .err()
        .ok_or("the lease should have failed")?;
    aca_error(error)
}

#[tokio::test]
async fn an_empty_workspace_lease_gets_a_session_that_cancel_deletes() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.provider.bind_worker(&WorkerId::new("worker-1")?)?;

    let grant = fixture
        .provider
        .lease(&Lease::empty().build()?, &lease_id("lease-1"))
        .await?;

    assert_eq!(fixture.provider.kind(), ProviderKind::AzureContainerApps);
    assert!(grant.sandbox_id.as_str().starts_with("aca-"));
    assert_eq!(grant.worker_id, Some(WorkerId::new("worker-1")?));
    assert_eq!(fixture.provider.active_lease_count().await, 1);
    assert_eq!(fixture.aca.executed().len(), 2);
    assert!(fixture.aca.executed()[0].contains("SANDBOX_ACA_IMAGE"));
    assert!(fixture.aca.executed()[1].contains("SANDBOX_ACA_IMAGE_TAG"));
    let events = fixture.emitter.events();
    assert!(matches!(events.first(), Some(LifecycleEvent::Started { .. })));
    assert!(matches!(events.last(), Some(LifecycleEvent::Completed { sandbox_id }) if *sandbox_id == grant.sandbox_id));

    fixture.provider.cancel(&lease_id("lease-1")).await?;
    fixture.provider.cancel(&lease_id("lease-1")).await?;

    assert_eq!(fixture.provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_lease_without_a_worker_binding_carries_no_worker() -> TestResult {
    let fixture = Fixture::new()?;

    let grant = fixture
        .provider
        .lease(&Lease::empty().build()?, &lease_id("lease-unbound"))
        .await?;

    assert_eq!(grant.worker_id, None);
    Ok(())
}

#[tokio::test]
async fn binding_a_second_worker_is_refused() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.provider.bind_worker(&WorkerId::new("worker-1")?)?;
    fixture.provider.bind_worker(&WorkerId::new("worker-1")?)?;

    let other = fixture.provider.bind_worker(&WorkerId::new("worker-2")?);

    assert!(other.is_err());
    Ok(())
}

#[tokio::test]
async fn a_snapshot_image_is_refused() -> TestResult {
    let fixture = Fixture::new()?;

    let error = failure(&fixture, Lease::empty().image(snapshot_image()), "lease-snapshot").await?;

    assert!(matches!(error, AcaError::SnapshotUnsupported(_)));
    Ok(())
}

#[tokio::test]
async fn env_mounts_and_host_mounts_are_refused() -> TestResult {
    let fixture = Fixture::new()?;
    let host = tempfile::tempdir()?;

    let env = failure(&fixture, Lease::empty().env(json!({ "KEY": "value" })), "lease-env").await?;
    let mounts = failure(&fixture, Lease::empty().with_mount(), "lease-mounts").await?;
    let host_mount = failure(&fixture, Lease::new(host_mount(host.path())), "lease-host").await?;

    assert!(matches!(env, AcaError::InvalidSpec(reason) if reason.contains("spec.env")));
    assert!(matches!(mounts, AcaError::InvalidSpec(reason) if reason.contains("mounts")));
    assert!(matches!(host_mount, AcaError::InvalidSpec(reason) if reason.contains("host_mount")));
    Ok(())
}

#[tokio::test]
async fn a_privileged_lease_is_refused_before_anything_is_allocated() -> TestResult {
    let fixture = Fixture::new()?;

    let error = fixture
        .provider
        .lease(&Lease::empty().privileged().build()?, &lease_id("lease-privileged"))
        .await
        .err()
        .ok_or("the lease should have failed")?;

    assert!(matches!(error, ProviderError::PrivilegedUnsupported(_)));
    assert!(fixture.emitter.events().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_lease_that_still_holds_a_session_cannot_be_allocated_again() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .provider
        .lease(&Lease::empty().build()?, &lease_id("lease-twice"))
        .await?;

    let error = failure(&fixture, Lease::empty(), "lease-twice").await?;

    assert!(matches!(error, AcaError::InvalidSpec(reason) if reason.contains("still holds")));
    assert_eq!(fixture.provider.active_lease_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_allocation_deletes_its_session_and_reports_failure() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.aca.fail_executions();

    let error = failure(&fixture, Lease::empty(), "lease-exec-fails").await?;

    assert!(matches!(error, AcaError::AcaApi { status: 500, .. }));
    assert_eq!(fixture.provider.active_lease_count().await, 0);
    assert!(matches!(
        fixture.emitter.events().last(),
        Some(LifecycleEvent::Failed { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_failed_upload_fails_the_allocation() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.aca.fail_uploads();
    let repo = tempfile::tempdir()?;
    std::fs::write(repo.path().join("a.txt"), "a")?;
    let source = clone_of(&format!("file://{}", repo.path().display()), "HEAD", no_auth());

    // A clone of a directory that is not a repository fails before any upload.
    let error = failure(&fixture, Lease::new(source), "lease-bad-clone").await?;

    assert!(matches!(error, AcaError::Workspace(_)));
    assert_eq!(fixture.provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_cloned_workspace_is_uploaded_file_by_file() -> TestResult {
    let fixture = Fixture::new()?;
    let repo = tempfile::tempdir()?;
    git(repo.path(), &["init", "-q", "-b", "main"])?;
    std::fs::create_dir(repo.path().join("src"))?;
    std::fs::write(repo.path().join("src/main.txt"), "hello")?;
    git(repo.path(), &["add", "-A"])?;
    git(repo.path(), &["commit", "-q", "-m", "init"])?;
    let source = clone_of(&format!("file://{}", repo.path().display()), "HEAD", no_auth());

    fixture
        .provider
        .lease(&Lease::new(source).build()?, &lease_id("lease-clone"))
        .await?;

    let uploads = fixture.aca.uploads();
    assert!(
        uploads
            .iter()
            .any(|(path, data)| path == "src/main.txt" && data == b"hello")
    );
    Ok(())
}

#[tokio::test]
async fn upload_failures_clean_the_session_up() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.aca.fail_uploads();
    let volumes = fixture.volume_root();
    std::fs::create_dir_all(volumes.join("data"))?;
    std::fs::write(volumes.join("data/file.txt"), "x")?;

    let error = failure(&fixture, Lease::new(volume("box", "data", "read_only")), "lease-upload").await?;

    assert!(matches!(error, AcaError::AcaApi { status: 500, .. }));
    assert_eq!(fixture.provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_read_write_volume_is_pulled_and_pushed_back_on_cancel() -> TestResult {
    let fixture = Fixture::new()?;
    let volumes = fixture.volume_root();
    std::fs::create_dir_all(volumes.join("data"))?;
    std::fs::write(volumes.join("data/file.txt"), "from the volume")?;

    fixture
        .provider
        .lease(
            &Lease::new(volume("box", "data", "read_write")).build()?,
            &lease_id("lease-rw"),
        )
        .await?;
    fixture.provider.cancel(&lease_id("lease-rw")).await?;

    assert!(
        fixture
            .aca
            .uploads()
            .iter()
            .any(|(path, data)| path == "file.txt" && data == b"from the volume")
    );
    let scopes = fixture.issuer.scopes();
    assert_eq!(scopes.len(), 2);
    assert!(scopes.iter().all(
        |scope| matches!(scope, CredentialScope::S3ReadWrite { bucket, prefix } if bucket == "box" && prefix == "data")
    ));
    Ok(())
}

#[tokio::test]
async fn a_read_only_volume_is_never_pushed() -> TestResult {
    let fixture = Fixture::new()?;
    let volumes = fixture.volume_root();
    std::fs::create_dir_all(volumes.join("data"))?;
    std::fs::write(volumes.join("data/file.txt"), "read me")?;

    fixture
        .provider
        .lease(
            &Lease::new(volume("box", "data", "read_only")).build()?,
            &lease_id("lease-ro"),
        )
        .await?;
    fixture.provider.cancel(&lease_id("lease-ro")).await?;

    assert_eq!(fixture.issuer.scopes().len(), 1);
    assert!(matches!(fixture.issuer.scopes()[0], CredentialScope::S3ReadOnly { .. }));
    Ok(())
}

#[tokio::test]
async fn a_volume_needs_a_storage_account() -> TestResult {
    let mut config = config("http://login.invalid")?;
    config.volume_storage_account = None;
    let fixture = Fixture::with_config(config)?;

    let error = failure(
        &fixture,
        Lease::new(volume("box", "data", "read_only")),
        "lease-no-account",
    )
    .await?;

    assert!(matches!(error, AcaError::InvalidSpec(reason) if reason.contains("STORAGE_ACCOUNT")));
    Ok(())
}

#[tokio::test]
async fn a_malformed_volume_credential_fails_the_allocation() -> TestResult {
    let fixture = Fixture::new()?;
    let issuer = support::fake_issuer::FakeIssuer::with_tokens("a:b:c", "no-colon-here");
    let provider = trogon_sandbox_aca_worker::AcaWorkerProvider::with_emitter(
        fixture.emitter.clone(),
        config("http://login.invalid")?,
        std::sync::Arc::new(issuer),
    )
    .with_client(fixture.aca.clone())
    .with_auth_bypass(trogon_sandbox_contract::ShortLivedToken::new("bypass"));

    let error = provider
        .lease(
            &Lease::new(volume("box", "data", "read_only")).build()?,
            &lease_id("lease-bad-credential"),
        )
        .await
        .err()
        .ok_or("the lease should have failed")?;

    assert!(matches!(aca_error(error)?, AcaError::VolumeCredentialFormat));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_session_that_never_becomes_ready_fails_the_allocation() -> TestResult {
    let fixture = Fixture::new()?;
    for _ in 0..60 {
        fixture
            .aca
            .script_get(Ok(SessionStatus::Pending("Creating".to_owned())));
    }

    let error = failure(&fixture, Lease::empty(), "lease-slow").await?;

    assert!(matches!(error, AcaError::SessionNotReady { attempts: 60 }));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_session_that_appears_late_is_waited_for() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.aca.script_get(Err(404));
    fixture
        .aca
        .script_get(Ok(SessionStatus::Pending("Creating".to_owned())));

    fixture
        .provider
        .lease(&Lease::empty().build()?, &lease_id("lease-late"))
        .await?;

    assert_eq!(fixture.provider.active_lease_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_cleanup_is_reported_and_retried_until_the_session_is_gone() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.aca.fail_executions();
    fixture.aca.script_delete(Err(500));

    let error = failure(&fixture, Lease::empty(), "lease-stuck").await?;

    assert!(matches!(error, AcaError::AllocationCleanup { .. }));
    assert_eq!(fixture.provider.active_lease_count().await, 1);

    fixture.aca.script_delete(Err(503));
    fixture.provider.retry_failed_allocations().await;
    assert_eq!(fixture.provider.active_lease_count().await, 1);

    fixture.provider.retry_failed_allocations().await;
    assert_eq!(fixture.provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_failed_cancel_keeps_the_lease_held() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .provider
        .lease(&Lease::empty().build()?, &lease_id("lease-cancel-fails"))
        .await?;
    fixture.aca.script_delete(Err(500));

    let error = fixture
        .provider
        .cancel(&lease_id("lease-cancel-fails"))
        .await
        .err()
        .ok_or("the cancel should have failed")?;

    assert!(matches!(aca_error(error)?, AcaError::AcaApi { status: 500, .. }));
    assert_eq!(fixture.provider.active_lease_count().await, 1);
    fixture.aca.script_delete(Ok(SessionDeleteOutcome::Absent));
    fixture.provider.cancel(&lease_id("lease-cancel-fails")).await?;
    assert_eq!(fixture.provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn the_bearer_comes_from_the_service_principal_exchange() -> TestResult {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/tenant/oauth2/v2.0/token"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({ "access_token": "exchanged" })))
        .mount(&server)
        .await;
    let fixture = Fixture::exchanging(&server.uri())?;

    fixture
        .provider
        .lease(&Lease::empty().ttl_seconds(120).build()?, &lease_id("lease-exchange"))
        .await?;
    fixture.provider.cancel(&lease_id("lease-exchange")).await?;

    assert!(fixture.aca.bearers().iter().all(|bearer| bearer == "exchanged"));
    assert_eq!(fixture.issuer.ttls(), vec![120, 300]);
    assert!(matches!(
        fixture.issuer.scopes()[0],
        CredentialScope::AzureServicePrincipal { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn a_broker_failure_fails_the_allocation() -> TestResult {
    let fixture = Fixture::exchanging("http://login.invalid")?;
    fixture.issuer.fail(CredentialProvider::Azure);

    let error = failure(&fixture, Lease::empty(), "lease-no-broker").await?;

    assert!(matches!(error, AcaError::Auth(_)));
    assert_eq!(fixture.provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_malformed_service_principal_fails_the_allocation() -> TestResult {
    let fixture = Fixture::exchanging("http://login.invalid")?;
    let issuer = support::fake_issuer::FakeIssuer::with_tokens("not-a-principal", "a:b");
    let provider = trogon_sandbox_aca_worker::AcaWorkerProvider::with_emitter(
        fixture.emitter.clone(),
        config("http://login.invalid")?,
        std::sync::Arc::new(issuer),
    )
    .with_client(std::sync::Arc::new(FakeAca::default()));

    let error = provider
        .lease(&Lease::empty().build()?, &lease_id("lease-bad-principal"))
        .await
        .err()
        .ok_or("the lease should have failed")?;

    assert!(matches!(aca_error(error)?, AcaError::Auth(_)));
    Ok(())
}

#[tokio::test]
async fn a_lease_without_a_ttl_cap_uses_the_configured_lifetime() -> TestResult {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({ "access_token": "t" })))
        .mount(&server)
        .await;
    let fixture = Fixture::exchanging(&server.uri())?;

    fixture
        .provider
        .lease(&Lease::empty().ttl_seconds(0).build()?, &lease_id("lease-zero"))
        .await?;

    assert_eq!(fixture.issuer.ttls(), vec![1]);
    tokio::time::timeout(Duration::from_secs(5), fixture.provider.cancel(&lease_id("lease-zero"))).await??;
    Ok(())
}

#[tokio::test]
async fn an_unusable_volume_store_fails_the_allocation() -> TestResult {
    let fixture = Fixture::with_unusable_volumes()?;

    let error = failure(
        &fixture,
        Lease::new(volume("box", "data", "read_only")),
        "lease-bad-store",
    )
    .await?;

    assert!(matches!(error, AcaError::Volume(_)));
    assert_eq!(fixture.provider.active_lease_count().await, 0);
    Ok(())
}
