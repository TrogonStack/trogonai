use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::StreamExt;
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderKind, ProviderLease, SandboxId, WorkerId,
};
use trogon_sandbox_protocol::{
    AttemptLedger, AuthorityHost, AuthorityHostError, AuthorityResourceId, FreezeOutcome, HeldLease,
    LedgerAuthorityHost, PurgeOutcome, ThawOutcome,
};

type TestResult = Result<(), Box<dyn Error>>;

fn open_ledger(dir: &tempfile::TempDir) -> Result<Arc<AttemptLedger>, Box<dyn Error>> {
    Ok(Arc::new(AttemptLedger::open(
        dir.path().join("attempts.sqlite3"),
        ProviderKind::Fly,
        None,
    )?))
}

fn identity_for(ledger: &AttemptLedger, lease: &str, attempt: &str) -> Result<LeaseAttemptIdentity, Box<dyn Error>> {
    Ok(LeaseAttemptIdentity {
        lease_id: LeaseId::new(lease),
        provider: ProviderKind::Fly,
        target: DispatchTarget {
            worker_id: ledger.worker_id().clone(),
            attempt_id: DispatchAttemptId::new(attempt)?,
            epoch: Some(LeaseEpoch::new(7)?),
        },
    })
}

async fn allocation(ledger: &AttemptLedger) -> Result<LeaseAttemptIdentity, Box<dyn Error>> {
    let identity = identity_for(ledger, "authority-lease", "authority-attempt")?;
    let sandbox_id = SandboxId::new("owned-machine")?;
    ledger
        .dispatch(&identity, || async {
            Ok::<_, std::io::Error>(ProviderLease {
                sandbox_id,
                evidence_subject: "sandbox.evidence.authority-lease".to_owned(),
                worker_id: None,
                attempt_id: None,
                epoch: None,
            })
        })
        .await?;
    Ok(identity)
}

fn cleaned(identity: &LeaseAttemptIdentity, intent: CancelIntent) -> ProviderCleanupResponse {
    ProviderCleanupResponse::Cleaned {
        identity: identity.clone(),
        intent,
    }
}

#[tokio::test]
async fn inventory_lists_started_attempts_and_a_failed_cleanup_is_retried() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open_ledger(&dir)?;
    let identity = allocation(&ledger).await?;
    let attempts = Arc::new(AtomicUsize::new(0));
    let backend_present = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let host = LedgerAuthorityHost::new(ledger.clone(), {
        let ledger = ledger.clone();
        let attempts = attempts.clone();
        let backend_present = backend_present.clone();
        move |request: ProviderCleanupRequest| {
            let ledger = ledger.clone();
            let attempts = attempts.clone();
            let backend_present = backend_present.clone();
            async move {
                Ok(ledger
                    .cleanup(&request, |_state| async move {
                        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                            return Err(std::io::Error::other("owned backend deletion failed"));
                        }
                        backend_present.store(false, Ordering::SeqCst);
                        Ok(())
                    })
                    .await)
            }
        }
    });
    let held = HeldLease::Targeted(identity);
    assert_eq!(host.held_leases().await?, vec![held.clone()]);
    assert_eq!(host.freeze(&held).await?, FreezeOutcome::Unsupported);
    assert_eq!(host.thaw(&held).await?, ThawOutcome::NoLongerOwned);

    let failure = host
        .purge(&held)
        .await
        .err()
        .ok_or("a failed teardown must fail the purge")?;
    assert!(
        matches!(&failure, AuthorityHostError::CleanupFailed { message } if message.contains("owned backend deletion failed")),
        "got {failure:?}"
    );
    assert!(backend_present.load(Ordering::SeqCst));
    assert_eq!(host.held_leases().await?, vec![held.clone()]);

    assert_eq!(host.purge(&held).await?, PurgeOutcome::Purged);
    assert!(!backend_present.load(Ordering::SeqCst));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(host.held_leases().await?.is_empty());

    assert_eq!(host.purge(&held).await?, PurgeOutcome::Settled);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn a_stale_epoch_a_foreign_worker_and_a_provider_resource_cannot_request_cleanup() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open_ledger(&dir)?;
    let identity = allocation(&ledger).await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let host = LedgerAuthorityHost::new(ledger, {
        let calls = calls.clone();
        move |request: ProviderCleanupRequest| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(cleaned(&request.identity, CancelIntent::Purge)) }
        }
    });

    let mut stale = identity.clone();
    stale.target.epoch = Some(LeaseEpoch::new(8)?);
    assert_eq!(host.purge(&HeldLease::Targeted(stale)).await?, PurgeOutcome::Stale);

    let mut foreign = identity.clone();
    foreign.target.worker_id = WorkerId::new("foreign-worker")?;
    assert_eq!(host.purge(&HeldLease::Targeted(foreign)).await?, PurgeOutcome::Stale);

    let mut other_provider = identity.clone();
    other_provider.provider = ProviderKind::Mock;
    assert_eq!(
        host.purge(&HeldLease::Targeted(other_provider)).await?,
        PurgeOutcome::Stale
    );

    let resource = HeldLease::Provider {
        lease_id: identity.lease_id.clone(),
        provider: ProviderKind::Fly,
        resource_id: AuthorityResourceId::new("owned-machine")?,
    };
    assert_eq!(host.purge(&resource).await?, PurgeOutcome::Stale);

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(host.held_leases().await?, vec![HeldLease::Targeted(identity)]);
    Ok(())
}

#[tokio::test]
async fn replies_that_do_not_confirm_the_exact_purge_are_rejected_until_one_does() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open_ledger(&dir)?;
    let identity = allocation(&ledger).await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let host = LedgerAuthorityHost::new(ledger.clone(), {
        let ledger = ledger.clone();
        let calls = calls.clone();
        move |request: ProviderCleanupRequest| {
            let ledger = ledger.clone();
            let step = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                Ok(match step {
                    0 => {
                        let mut wrong = request.identity;
                        wrong.target.epoch =
                            Some(LeaseEpoch::new(8).map_err(|error| AuthorityHostError::Failed(Box::new(error)))?);
                        cleaned(&wrong, CancelIntent::Purge)
                    }
                    1 => cleaned(&request.identity, CancelIntent::Release),
                    2 => {
                        let mut wrong = request.identity;
                        wrong.target.epoch = Some(LeaseEpoch::FIRST);
                        ProviderCleanupResponse::Failed {
                            identity: wrong,
                            message: "for another assignment".to_owned(),
                        }
                    }
                    3 => cleaned(&request.identity, CancelIntent::Purge),
                    _ => {
                        ledger
                            .cleanup(&request, |_| async { Ok::<_, std::io::Error>(()) })
                            .await
                    }
                })
            }
        }
    });
    let held = HeldLease::Targeted(identity);

    for expected_calls in 1..=3 {
        let error = host
            .purge(&held)
            .await
            .err()
            .ok_or("a reply that does not confirm the exact purge must fail")?;
        assert!(
            matches!(error, AuthorityHostError::CleanupReplyMismatch),
            "got {error:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        assert_eq!(host.held_leases().await?, vec![held.clone()]);
    }

    let unrecorded = host
        .purge(&held)
        .await
        .err()
        .ok_or("a reply the ledger does not record must fail")?;
    assert!(
        matches!(unrecorded, AuthorityHostError::ConfirmationMissing),
        "got {unrecorded:?}"
    );
    assert_eq!(host.held_leases().await?, vec![held.clone()]);

    assert_eq!(host.purge(&held).await?, PurgeOutcome::Purged);
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    Ok(())
}

#[tokio::test]
async fn a_failure_reply_for_the_exact_assignment_carries_the_provider_message() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open_ledger(&dir)?;
    let identity = allocation(&ledger).await?;
    let host = LedgerAuthorityHost::new(ledger, |request: ProviderCleanupRequest| async move {
        Ok(ProviderCleanupResponse::Failed {
            identity: request.identity,
            message: "provider refused".to_owned(),
        })
    });
    let error = host
        .purge(&HeldLease::Targeted(identity))
        .await
        .err()
        .ok_or("a failure reply must fail the purge")?;
    assert!(
        matches!(&error, AuthorityHostError::CleanupFailed { message } if message == "provider refused"),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_cleanup_callback_error_is_passed_through() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open_ledger(&dir)?;
    let identity = allocation(&ledger).await?;
    let host = LedgerAuthorityHost::new(ledger, |_: ProviderCleanupRequest| async {
        Err::<ProviderCleanupResponse, _>(AuthorityHostError::Failed("transport down".into()))
    });
    let error = host
        .purge(&HeldLease::Targeted(identity))
        .await
        .err()
        .ok_or("a callback error must fail the purge")?;
    assert!(matches!(&error, AuthorityHostError::Failed(_)), "got {error:?}");
    assert_eq!(error.to_string(), "transport down");
    Ok(())
}

#[tokio::test]
async fn an_inventory_failure_cannot_be_reported_as_an_empty_worker() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path: PathBuf = dir.path().join("attempts.sqlite3");
    let ledger = open_ledger(&dir)?;
    let identity = allocation(&ledger).await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let host = LedgerAuthorityHost::new(ledger, {
        let calls = calls.clone();
        move |_: ProviderCleanupRequest| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<ProviderCleanupResponse, _>(AuthorityHostError::Failed("cleanup is unavailable".into())) }
        }
    });
    std::fs::rename(&path, path.with_extension("unavailable"))?;

    let inventory = host
        .held_leases()
        .await
        .err()
        .ok_or("an unavailable ledger must fail the inventory")?;
    assert!(matches!(inventory, AuthorityHostError::Ledger(_)), "got {inventory:?}");
    let purge = host
        .purge(&HeldLease::Targeted(identity))
        .await
        .err()
        .ok_or("an unavailable ledger must fail the purge")?;
    assert!(matches!(purge, AuthorityHostError::Ledger(_)), "got {purge:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn an_unknown_assignment_has_no_cleanup_or_settlement_authority() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open_ledger(&dir)?;
    let identity = identity_for(&ledger, "unrecorded-lease", "unrecorded-attempt")?;
    let calls = Arc::new(AtomicUsize::new(0));
    let host = LedgerAuthorityHost::new(ledger, {
        let calls = calls.clone();
        move |request: ProviderCleanupRequest| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(cleaned(&request.identity, CancelIntent::Purge)) }
        }
    });
    assert_eq!(host.purge(&HeldLease::Targeted(identity)).await?, PurgeOutcome::Stale);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(host.held_leases().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_attempt_whose_allocation_never_started_is_not_held() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open_ledger(&dir)?;
    let identity = identity_for(&ledger, "never-started", "never-started-attempt")?;
    let response = ledger
        .cleanup(
            &ProviderCleanupRequest {
                identity,
                intent: CancelIntent::Release,
            },
            |_| async { Ok::<_, std::io::Error>(()) },
        )
        .await;
    assert!(matches!(response, ProviderCleanupResponse::Cleaned { .. }));
    let host = LedgerAuthorityHost::new(ledger, |request: ProviderCleanupRequest| async move {
        Ok(cleaned(&request.identity, CancelIntent::Purge))
    });
    assert!(host.held_leases().await?.is_empty());
    Ok(())
}

struct Server {
    child: std::process::Child,
    _dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn server_binary() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("nats-server"))
            .find(|candidate| candidate.is_file())
    });
    on_path.or_else(|| {
        let home = std::env::var_os("HOME")?;
        let shim = PathBuf::from(home).join(".local/share/mise/shims/nats-server");
        shim.is_file().then_some(shim)
    })
}

/// A private NATS server and a client, or `None` when no server can be run here.
async fn nats() -> Result<Option<(Server, async_nats::Client)>, Box<dyn Error>> {
    let Some(binary) = server_binary() else {
        return Ok(None);
    };
    let dir = tempfile::tempdir()?;
    let Ok(child) = std::process::Command::new(binary)
        .args(["-a", "127.0.0.1", "-p", "-1", "--ports_file_dir"])
        .arg(dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return Ok(None);
    };
    let ports_file = dir.path().join(format!("nats-server_{}.ports", child.id()));
    let mut server = Server { child, _dir: dir };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if server.child.try_wait()?.is_some() {
            return Ok(None);
        }
        let ports = std::fs::read(&ports_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        if let Some(url) = ports.as_ref().and_then(|ports| ports["nats"][0].as_str())
            && let Ok(client) = async_nats::connect(url).await
        {
            return Ok(Some((server, client)));
        }
        if std::time::Instant::now() > deadline {
            return Err("the private NATS server did not become ready".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn cleanup_requested_over_nats_purges_and_then_reports_settled() -> TestResult {
    let Some((server, client)) = nats().await? else {
        return Ok(());
    };
    let dir = tempfile::tempdir()?;
    let ledger = open_ledger(&dir)?;
    let identity = allocation(&ledger).await?;
    let subject = "sandbox.test.cleanup.owner".to_owned();
    let mut requests = client.subscribe(subject.clone()).await?;
    client.flush().await?;
    let responder = tokio::spawn({
        let ledger = ledger.clone();
        let client = client.clone();
        async move {
            while let Some(message) = requests.next().await {
                let Some(reply) = message.reply else { continue };
                let Ok(request) = serde_json::from_slice::<ProviderCleanupRequest>(&message.payload) else {
                    continue;
                };
                let response = ledger
                    .cleanup(&request, |_| async { Ok::<_, std::io::Error>(()) })
                    .await;
                let Ok(body) = serde_json::to_vec(&response) else {
                    continue;
                };
                let _ = client.publish(reply, body.into()).await;
            }
        }
    });
    let host = LedgerAuthorityHost::new(ledger, move |request: ProviderCleanupRequest| {
        let client = client.clone();
        let subject = subject.clone();
        async move {
            let body = serde_json::to_vec(&request).map_err(|error| AuthorityHostError::Failed(Box::new(error)))?;
            let reply = client
                .request(subject, body.into())
                .await
                .map_err(|error| AuthorityHostError::Failed(Box::new(error)))?;
            serde_json::from_slice(&reply.payload).map_err(|error| AuthorityHostError::Failed(Box::new(error)))
        }
    });
    let held = HeldLease::Targeted(identity);
    assert_eq!(host.held_leases().await?, vec![held.clone()]);
    assert_eq!(host.purge(&held).await?, PurgeOutcome::Purged);
    assert!(host.held_leases().await?.is_empty());
    assert_eq!(host.purge(&held).await?, PurgeOutcome::Settled);
    responder.abort();
    drop(server);
    Ok(())
}
