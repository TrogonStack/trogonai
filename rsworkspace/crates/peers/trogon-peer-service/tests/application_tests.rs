use std::sync::atomic::Ordering;
use trogon_peer::{MessageId, PeerId};
use trogon_peer_service::WorkLimit;
mod support;
use support::*;
use trogonai_proto::peers::v1alpha1 as proto;

#[tokio::test]
async fn durable_acceptance_survives_reconstruction_and_exact_retry_with_new_content_location() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f
        .app()
        .send(&f.alice, f.send("key", "hello <peer> & \"world\""))
        .await?;
    let retry = f
        .app()
        .send(&f.alice, f.send("key", "hello <peer> & \"world\""))
        .await?;
    assert_eq!(sent, retry);
    let status = f
        .app()
        .status(
            &f.alice,
            proto::StatusRequest {
                attachment: f.a.clone().into(),
                idempotency_key: Some("key".into()),
            },
        )
        .await?;
    assert_eq!(status.message, sent.message);
    assert_eq!(
        status.status.and_then(|v| v.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_ACCEPTED)
    );
    assert!(f.app().send(&f.alice, f.send("key", "different")).await.is_err());
    Ok(())
}

#[tokio::test]
async fn denied_acceptance_touches_no_store_content_cache_or_catalog() -> TestResult {
    let f = Fixture::new().await?;
    let before = (
        f.store.reads.load(Ordering::SeqCst),
        f.store.appends.load(Ordering::SeqCst),
    );
    assert!(f.app().send(&f.bob, f.send("denied", "private")).await.is_err());
    assert_eq!(
        before,
        (
            f.store.reads.load(Ordering::SeqCst),
            f.store.appends.load(Ordering::SeqCst)
        )
    );
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn explicit_reconciliation_intakes_from_trusted_history_without_settling_early() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f.app().send(&f.alice, f.send("key", "pending")).await?;
    let id = MessageId::new(
        sent.message
            .message_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    f.app()
        .reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
        .await?;
    f.app().reconcile(&f.alice, &sender, &[id], WorkLimit::new(1)?).await?;
    let status = f
        .app()
        .status(
            &f.alice,
            proto::StatusRequest {
                attachment: f.a.clone().into(),
                idempotency_key: Some("key".into()),
            },
        )
        .await?;
    assert_eq!(
        status.status.and_then(|v| v.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_INTAKE_RECORDED)
    );
    Ok(())
}

#[tokio::test]
async fn prepared_assignment_replays_cached_exact_payload_and_frame_over_reconstruction() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("key", "<untrusted> & \"quoted\"").await?;
    let first = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert!(first.batch.is_set());
    assert_eq!(first.messages[0].text.as_deref(), Some("<untrusted> & \"quoted\""));
    let cached = f
        .cache
        .records
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .values()
        .next()
        .cloned()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    assert!(cached.frame().contains("\\u003c"));
    f.content
        .bodies
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .clear();
    let replay = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(first, replay);
    Ok(())
}

#[tokio::test]
async fn cache_failure_does_not_commit_preparation_and_frame_corruption_fails_closed() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("key", "payload").await?;
    let before = f.store.appends.load(Ordering::SeqCst);
    f.cache.fail_insert.store(true, Ordering::SeqCst);
    assert!(f.app().prepare_batch(&f.bob, f.prepare()).await.is_err());
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before);
    f.cache.fail_insert.store(false, Ordering::SeqCst);
    let first = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    {
        let mut records = f
            .cache
            .records
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?;
        for record in records.values_mut() {
            *record = trogon_peer_service::PreparedPresentation::from_port(
                record.batch().clone(),
                record.messages().to_vec(),
                "incompatible frame".into(),
            );
        }
    }
    let unavailable = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(unavailable.batch, first.batch);
    assert!(unavailable.messages.is_empty());
    assert_eq!(
        unavailable.content_availability.and_then(|v| v.as_known()),
        Some(proto::ContentAvailability::CONTENT_AVAILABILITY_RETRY_UNAVAILABLE)
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before + 1);
    Ok(())
}

#[tokio::test]
async fn escaped_rendering_selects_fitting_oldest_prefix_before_preparation() -> TestResult {
    let f = Fixture::new().await?;
    let first = f.intake("first", &"<".repeat(900)).await?;
    f.intake("second", &"<".repeat(900)).await?;
    let prepared = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.messages[0].metadata, first.message);
    Ok(())
}

struct ExpireDuringContentOpen {
    inner: DisposableContentFixture,
    clock: ClockFixture,
    expiry: chrono::DateTime<chrono::Utc>,
}

impl trogon_peer_service::ContentOpen for ExpireDuringContentOpen {
    async fn open(
        &self,
        scope: &trogon_peer_service::PeerAccess,
        binding: &trogon_peer_service::ContentBinding,
    ) -> Result<Option<trogon_peer_service::MessageText>, trogon_peer_service::PeerPortError> {
        let text = trogon_peer_service::ContentOpen::open(&self.inner, scope, binding).await?;
        tokio::task::yield_now().await;
        *self
            .clock
            .time
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)? = self.expiry;
        Ok(text)
    }
}

async fn replay_reconciliation_receipt(
    store: &StoreFixture,
    peer: &PeerId,
) -> TestResult<trogonai_proto::peers::state_v1alpha1::PeerState> {
    use trogon_decider_runtime::StreamRead;
    let history = store
        .read_stream(trogon_decider_runtime::ReadStreamRequest {
            stream_id: peer,
            from: trogon_decider_runtime::ReadFrom::Beginning,
        })
        .await?;
    let mut state = trogon_peer::commands::state::initial_state();
    for event in history.events {
        let event = event
            .decode::<proto::PeerEvent>()?
            .into_decoded()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
        state = trogon_peer::commands::state::evolve(state, &event)?;
    }
    Ok(state)
}

#[tokio::test]
async fn reconciliation_crossing_expiry_during_content_open_records_terminal_receipt() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f.app().send(&f.alice, f.send("expiry-race", "pending")).await?;
    let id = MessageId::new(
        sent.message
            .message_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let recipient = PeerId::parse(
        f.b.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let expiry = trogonai_proto::convert::datetime_from_timestamp(
        sent.message
            .expires_at
            .as_option()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let content = ExpireDuringContentOpen {
        inner: f.content.clone(),
        clock: f.clock.clone(),
        expiry,
    };
    let app = trogon_peer_service::PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        content,
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );

    assert_eq!(
        app.reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
            .await?,
        1
    );
    let target = replay_reconciliation_receipt(&f.store, &recipient).await?;
    assert!(target.inbox.is_empty(), "expired content must not be intaken");
    assert_eq!(target.dispositions.len(), 1);
    let receipt = target
        .dispositions
        .first()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    assert_eq!(receipt.message_id.as_deref(), Some(id.as_str()));
    assert_eq!(
        receipt.outcome.and_then(|value| value.as_known()),
        Some(proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED)
    );
    let source = replay_reconciliation_receipt(&f.store, &sender).await?;
    let settled = source
        .outbox
        .first()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    assert_eq!(settled.settlement.as_option(), Some(receipt));
    Ok(())
}

#[tokio::test]
async fn preparation_accepts_absent_and_empty_optional_prompt_correlation() -> TestResult {
    for prompt in [None, Some(String::new())] {
        let f = Fixture::new().await?;
        f.intake("optional-prompt", "retained context").await?;
        let mut request = f.prepare();
        request.prompt_id = prompt;
        let prepared = f.app().prepare_batch(&f.bob, request.clone()).await?;
        assert_eq!(prepared.messages.len(), 1);
        assert_eq!(prepared.messages[0].text.as_deref(), Some("retained context"));
        assert_eq!(f.app().prepare_batch(&f.bob, request).await?, prepared);
    }
    Ok(())
}

#[tokio::test]
async fn malformed_prompt_correlation_uses_identity_errors_before_port_access() -> TestResult {
    let f = Fixture::new().await?;
    let mut request = f.prepare();
    request.prompt_id = Some("prompt\ninvalid".into());
    let reads = f.store.reads.load(Ordering::SeqCst);
    assert!(matches!(
        f.app().prepare_batch(&f.bob, request).await,
        Err(trogon_peer_service::PeerApplicationError::Value(
            trogon_peer::PeerValueError::InvalidCharacter
        ))
    ));
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        trogon_peer_service::PromptId::new(String::new()),
        Err(trogon_peer::PeerValueError::Empty)
    ));
    assert!(matches!(
        trogon_peer_service::PromptId::new("x".repeat(129)),
        Err(trogon_peer::PeerValueError::TooLong { maximum: 128 })
    ));
    Ok(())
}

#[derive(Clone, Copy)]
enum WinningPresentationKey {
    Candidate,
    Distinct,
}

struct ConcurrentPreparationCache {
    inner: PresentationFixture,
    store: StoreFixture,
    policy: PolicyFixture,
    owner: trogon_peer::TrustedPeerOwner,
    attachment: trogon_peer::AttachmentRef,
    mode: WinningPresentationKey,
}

impl trogon_peer_service::PresentationRead for ConcurrentPreparationCache {
    async fn read(
        &self,
        key: &trogon_peer_service::PresentationKey,
    ) -> Result<Option<trogon_peer_service::PreparedPresentation>, trogon_peer_service::PeerPortError> {
        trogon_peer_service::PresentationRead::read(&self.inner, key).await
    }
}

impl trogon_peer_service::PresentationOrphan for ConcurrentPreparationCache {
    async fn orphan(
        &self,
        key: &trogon_peer_service::PresentationKey,
    ) -> Result<(), trogon_peer_service::PeerPortError> {
        trogon_peer_service::PresentationOrphan::orphan(&self.inner, key).await?;
        self.inner
            .records
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .remove(&PresentationFixture::key(key));
        Ok(())
    }
}

impl trogon_peer_service::PresentationInsert for ConcurrentPreparationCache {
    async fn insert(
        &self,
        key: &trogon_peer_service::PresentationKey,
        record: &trogon_peer_service::PreparedPresentation,
    ) -> Result<(), trogon_peer_service::PeerPortError> {
        use trogon_peer_service::PeerPortError;
        self.inner.insert(key, record).await?;
        let batch_id = match self.mode {
            WinningPresentationKey::Candidate => key.batch().clone(),
            WinningPresentationKey::Distinct => {
                trogon_peer::BatchId::new("competing-winner").map_err(|_| PeerPortError::Integrity)?
            }
        };
        let mut batch = record.batch().clone();
        batch.batch_id = Some(batch_id.as_str().to_owned());
        let frame = trogon_peer_context::PeerContext::render(&batch, record.messages())
            .map_err(|_| PeerPortError::Integrity)?;
        let winning_key =
            trogon_peer_service::PresentationKey::new(key.tenant().clone(), key.peer().clone(), batch_id.clone());
        let winning_record = trogon_peer_service::PreparedPresentation::from_port(
            batch.clone(),
            record.messages().to_vec(),
            frame.as_str().to_owned(),
        );
        self.inner.insert(&winning_key, &winning_record).await?;
        let plan = trogon_peer::TrustedBatchPlan::from_renderer(
            batch
                .message_ids
                .iter()
                .cloned()
                .map(MessageId::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| PeerPortError::Integrity)?,
            frame.as_str().len(),
        )
        .map_err(|_| PeerPortError::Integrity)?;
        let now = trogonai_proto::convert::datetime_from_timestamp(
            batch.prepared_at.as_option().ok_or(PeerPortError::Integrity)?,
        )
        .map_err(|_| PeerPortError::Integrity)?;
        let command = trogon_peer::PrepareBatch::new(self.owner.clone(), self.attachment.clone(), batch_id, plan, now);
        let principal = trogon_decider_runtime::CommandPrincipal::new(
            trogon_decider_runtime::PrincipalKind::Agent,
            trogon_decider_runtime::PrincipalId::new(self.owner.caller().as_str())
                .map_err(|_| PeerPortError::Integrity)?,
        );
        trogon_decider_runtime::CommandExecution::new(&self.store, &command)
            .with_principal(principal)
            .with_authorizer(&self.policy)
            .with_replay_limit(
                trogon_decider_runtime::ReplayLimit::try_new(4096).map_err(|_| PeerPortError::Integrity)?,
            )
            .execute()
            .await
            .map_err(|_| PeerPortError::Integrity)?;
        Ok(())
    }
}

#[tokio::test]
async fn preparation_race_retains_the_durable_winning_presentation() -> TestResult {
    for mode in [WinningPresentationKey::Candidate, WinningPresentationKey::Distinct] {
        let f = Fixture::new().await?;
        f.intake("cache-race", "winning context").await?;
        let attachment = trogon_peer::AttachmentRef::new(
            PeerId::parse(
                f.b.peer_id
                    .clone()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
            )?,
            trogon_peer::ConnectorId::new(
                f.b.connector_id
                    .clone()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
            )?,
            f.b.generation.ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?;
        let cache = ConcurrentPreparationCache {
            inner: f.cache.clone(),
            store: f.store.clone(),
            policy: f.policy.clone(),
            owner: f.bob.clone(),
            attachment,
            mode,
        };
        let app = trogon_peer_service::PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            f.clock.clone(),
            f.content.clone(),
            cache,
            f.ids.clone(),
            f.catalog.clone(),
        );

        let prepared = app.prepare_batch(&f.bob, f.prepare()).await?;
        assert_eq!(
            prepared.messages.len(),
            1,
            "the real winning command's cache must survive a competing preparation"
        );
        assert_eq!(prepared.messages[0].text.as_deref(), Some("winning context"));
        let batch_id = prepared
            .batch
            .batch_id
            .as_deref()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
        let key = format!(
            "{}:{}:{}",
            f.bob.tenant().as_str(),
            f.b.peer_id
                .as_deref()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
            batch_id
        );
        {
            let orphans = f
                .cache
                .orphans
                .lock()
                .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?;
            assert!(!orphans.contains(&key), "a durable winning key cannot be orphaned");
            assert_eq!(
                orphans.len(),
                match mode {
                    WinningPresentationKey::Candidate => 0,
                    WinningPresentationKey::Distinct => 1,
                }
            );
        }
        assert_eq!(f.app().prepare_batch(&f.bob, f.prepare()).await?, prepared);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum LostPreparationReply {
    #[error("append committed but its response was lost")]
    Committed,
    #[error("fixture append failed")]
    Inner(#[source] <StoreFixture as trogon_decider_runtime::StreamAppend<PeerId>>::Error),
}

struct CommitThenLoseReplyStore {
    inner: StoreFixture,
    lose_reply: std::sync::atomic::AtomicBool,
}

impl trogon_peer_service::ScopedPeerStore for CommitThenLoseReplyStore {
    fn tenant(&self) -> &trogon_peer::TenantId {
        &self.inner.tenant
    }
}

impl trogon_decider_runtime::StreamRead<PeerId> for CommitThenLoseReplyStore {
    type Error = <StoreFixture as trogon_decider_runtime::StreamRead<PeerId>>::Error;
    async fn read_stream(
        &self,
        request: trogon_decider_runtime::ReadStreamRequest<'_, PeerId>,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        trogon_decider_runtime::StreamRead::read_stream(&self.inner, request).await
    }
    async fn read_stream_bounded(
        &self,
        request: trogon_decider_runtime::ReadStreamRequest<'_, PeerId>,
        maximum: u64,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        trogon_decider_runtime::StreamRead::read_stream_bounded(&self.inner, request, maximum).await
    }
}

impl trogon_decider_runtime::StreamAppend<PeerId> for CommitThenLoseReplyStore {
    type Error = LostPreparationReply;
    async fn append_stream(
        &self,
        request: trogon_decider_runtime::AppendStreamRequest<'_, PeerId>,
    ) -> Result<trogon_decider_runtime::AppendStreamResponse, Self::Error> {
        let result = trogon_decider_runtime::StreamAppend::append_stream(&self.inner, request)
            .await
            .map_err(LostPreparationReply::Inner)?;
        if self.lose_reply.swap(false, Ordering::SeqCst) {
            return Err(LostPreparationReply::Committed);
        }
        Ok(result)
    }
    fn classify_append_failure(&self, error: &Self::Error) -> trogon_decider_runtime::AppendFailure {
        match error {
            LostPreparationReply::Committed => trogon_decider_runtime::AppendFailure::Fatal,
            LostPreparationReply::Inner(error) => {
                trogon_decider_runtime::StreamAppend::classify_append_failure(&self.inner, error)
            }
        }
    }
}

#[tokio::test]
async fn uncertain_preparation_append_retains_context_for_durable_retry() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("lost-prepare-reply", "durably prepared context").await?;
    let app = trogon_peer_service::PeerApplication::new(
        CommitThenLoseReplyStore {
            inner: f.store.clone(),
            lose_reply: std::sync::atomic::AtomicBool::new(true),
        },
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    assert!(
        app.prepare_batch(&f.bob, f.prepare()).await.is_err(),
        "lost reply must remain an uncertain operation error"
    );
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .is_empty(),
        "an append error cannot establish that the durable candidate lost"
    );
    let replay = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(replay.messages.len(), 1);
    assert_eq!(replay.messages[0].text.as_deref(), Some("durably prepared context"));
    let before = f.store.appends.load(Ordering::SeqCst);
    assert_eq!(f.app().prepare_batch(&f.bob, f.prepare()).await?, replay);
    assert_eq!(
        f.store.appends.load(Ordering::SeqCst),
        before,
        "read retry must not append a second preparation"
    );
    Ok(())
}
