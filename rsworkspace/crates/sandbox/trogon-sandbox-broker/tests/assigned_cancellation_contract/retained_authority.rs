use super::*;
use trogon_sandbox_broker::command_launch::{
    CommandLaunch, CommandLaunchStore, InMemoryCommandLaunchStore, LaunchState, fingerprint,
};
use trogon_sandbox_contract::DispatchAttemptId;

async fn fixture() -> TestResult<(Harness, LeaseRecord, Arc<InMemoryCommandLaunchStore>)> {
    let launches = Arc::new(InMemoryCommandLaunchStore::new());
    let harness = Harness::with_launch_store(Some(launches.clone())).await?;
    let worker = WorkerId::new("retained-owner")?;
    harness.heartbeat(&worker, true).await?;
    let pending = harness.begin_lease(&worker, "retained-authority-lease").await?;
    let (_, record) = harness.finish_lease(pending).await?;
    Ok((harness, record, launches))
}

pub(super) async fn start(
    harness: &Harness,
    record: &LeaseRecord,
    key: &str,
    strong: bool,
) -> TestResult<RunCommandResponse> {
    let mut runs = harness
        .client
        .subscribe(subjects::provider_run_command(PROVIDER, &record.lease_id))
        .await?;
    harness.client.flush().await?;
    let readiness = if strong {
        Some(
            harness
                .subscribe_readiness(&Harness::readiness_binding(record)?)
                .await?,
        )
    } else {
        None
    };
    let task = {
        let core = harness.core.clone();
        let mut request = command(&record.lease_id, key)?;
        request.require_launch_cancellation = strong.into();
        tokio::spawn(async move { core.handle_run_command(request).await })
    };
    if let Some(readiness) = readiness {
        harness
            .complete_readiness(readiness, &Harness::readiness_binding(record)?)
            .await?;
    }
    let observed = tokio::time::timeout(WAIT, runs.next())
        .await
        .context("the launch reaches its worker")?
        .context("the run subscription ended")?;
    harness.verify_owned_verb(&observed, record)?;
    let request: RunCommandRequest = serde_json::from_slice(&observed.payload)?;
    assert_eq!(request.launch_key.as_ref().map(LaunchKey::as_str), Some(key));
    assert_eq!(request.require_launch_cancellation.is_required(), strong);
    let response: RunCommandResponse = serde_json::from_value(json!({
        "outcome":"started", "execution_id":format!("synthetic-execution-{key}"),
        "origin":"created",
    }))?;
    harness
        .client
        .publish(
            observed.reply.context("the run carries a reply subject")?,
            serde_json::to_vec(&response)?.into(),
        )
        .await?;
    assert_eq!(
        tokio::time::timeout(WAIT, task).await.context("the launch settles")??,
        response
    );
    Ok(response)
}

async fn observer(harness: &Harness) -> TestResult<Subscriber> {
    let observer = harness
        .client
        .subscribe("sandbox.provider.*.cancel_command_launch.*")
        .await?;
    harness.client.flush().await?;
    Ok(observer)
}

async fn refuse(
    harness: &Harness,
    record: &LeaseRecord,
    key: &str,
    observer: &mut Subscriber,
    reason: &str,
) -> TestResult {
    let response = harness
        .core
        .handle_cancel_command_launch(cancellation(&record.lease_id, key)?);
    tokio::pin!(response);
    let response = tokio::time::timeout(WAIT, async {
        tokio::select! {
            response = &mut response => Ok(response),
            message = observer.next() => Err(format!("{reason}: unauthorized retained route {:?}", message.map(|msg| msg.subject))),
        }
    })
    .await
    .context("the refusal settles")??;
    assert!(
        matches!(
            response,
            CancelCommandLaunchResponse::Unsupported | CancelCommandLaunchResponse::Denied(_)
        ),
        "{reason}: retained cancellation must refuse without matching durable authority: {response:?}"
    );
    no_message(observer, reason).await;
    Ok(())
}

/// One way a stored assignment can differ from the assignment that owns a launch.
#[derive(Clone, Copy, Debug)]
enum Change {
    Worker,
    Attempt,
    Epoch,
    Sandbox,
    Tenant,
    Provider,
    MismatchedEpoch,
    MissingWorker,
    MissingAttempt,
    MissingEpoch,
}

impl Change {
    const ALL: [Self; 10] = [
        Self::Worker,
        Self::Attempt,
        Self::Epoch,
        Self::Sandbox,
        Self::Tenant,
        Self::Provider,
        Self::MismatchedEpoch,
        Self::MissingWorker,
        Self::MissingAttempt,
        Self::MissingEpoch,
    ];

    fn apply(self, record: &LeaseRecord) -> TestResult<LeaseRecord> {
        let mut changed = record.clone();
        let second = LeaseEpoch::new(2)?;
        let attempt = changed
            .dispatch_attempt
            .as_mut()
            .context("the owning record names its attempt")?;
        match self {
            Self::Worker => changed.worker_id = Some(WorkerId::new("different-worker")?),
            Self::Attempt => attempt.id = DispatchAttemptId::new("different-attempt")?,
            Self::Epoch => {
                changed.epoch = Some(second);
                attempt.epoch = Some(second);
            }
            Self::Sandbox => changed.sandbox_id = "different-native-receipt".into(),
            Self::Tenant => changed.tenant = TenantId::new("different-tenant")?,
            Self::Provider => changed.provider = ProviderKind::E2b,
            Self::MismatchedEpoch => attempt.epoch = Some(second),
            Self::MissingWorker => changed.worker_id = None,
            Self::MissingAttempt => changed.dispatch_attempt = None,
            Self::MissingEpoch => {
                changed.epoch = None;
                attempt.epoch = None;
            }
        }
        Ok(changed)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_authority_cannot_cross_a_changed_assignment_or_native_receipt() -> TestResult {
    let (harness, record, _) = fixture().await?;
    start(&harness, &record, "owned-launch", true).await?;
    harness.heartbeat(assigned_worker(&record)?, false).await?;
    let mut observer = observer(&harness).await?;
    for change in Change::ALL {
        let changed = change.apply(&record)?;
        harness.active.put(&changed).await?;
        refuse(&harness, &record, "owned-launch", &mut observer, &format!("{change:?}")).await?;
    }
    harness.active.put(&record).await?;
    eprintln!(
        "retained broker authority negatives: healthy_signed_start=true changed_dimensions={} provider_delivery=0 worker_is_stub=true native_acceptance=false",
        Change::ALL.len()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_and_ordinary_launch_records_do_not_fabricate_retained_strong_authority() -> TestResult {
    let (harness, record, launches) = fixture().await?;
    let response = start(&harness, &record, "owned-control", true).await?;
    start(&harness, &record, "ordinary-launch", false).await?;
    let legacy = CommandLaunch::new(
        fingerprint(&command(&record.lease_id, "legacy-launch")?)?,
        LaunchState::Done { response },
    );
    launches
        .finish(&record.lease_id, &LaunchKey::new("legacy-launch")?, &legacy)
        .await?;
    harness.heartbeat(assigned_worker(&record)?, false).await?;
    let mut observer = observer(&harness).await?;
    for key in ["legacy-launch", "ordinary-launch"] {
        refuse(&harness, &record, key, &mut observer, key).await?;
    }
    Ok(())
}
