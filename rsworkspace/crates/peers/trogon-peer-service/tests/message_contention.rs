use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, AppendStreamResponse, ReadStreamRequest, ReadStreamResponse, StreamAppend,
    StreamRead,
};
use trogon_peer_service::{PeerApplication, PeerClock, PeerPortError, PeerService};
use trogonai_proto::peers::v1alpha1 as proto;
mod support;
use support::*;

#[derive(Debug, thiserror::Error)]
enum InterleavingError {
    #[error("test interleaving could not establish its precondition")]
    Invariant,
    #[error(transparent)]
    Store(<StoreFixture as StreamAppend<trogon_peer::PeerId>>::Error),
}

#[derive(Clone)]
struct HeartbeatBeforeMessageAppend {
    inner: StoreFixture,
    policy: PolicyFixture,
    clock: ClockFixture,
    owner: trogon_peer::TrustedPeerOwner,
    reference: proto::AttachmentRef,
    remaining: Arc<AtomicUsize>,
}

impl trogon_peer_service::ScopedPeerStore for HeartbeatBeforeMessageAppend {
    fn tenant(&self) -> &trogon_peer::TenantId {
        &self.inner.tenant
    }
}

impl StreamRead<trogon_peer::PeerId> for HeartbeatBeforeMessageAppend {
    type Error = <StoreFixture as StreamRead<trogon_peer::PeerId>>::Error;

    async fn read_stream(
        &self,
        request: ReadStreamRequest<'_, trogon_peer::PeerId>,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream(request).await
    }

    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, trogon_peer::PeerId>,
        maximum: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream_bounded(request, maximum).await
    }
}

impl StreamAppend<trogon_peer::PeerId> for HeartbeatBeforeMessageAppend {
    type Error = InterleavingError;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, trogon_peer::PeerId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        if self
            .remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| count.checked_sub(1))
            .is_ok()
        {
            {
                let mut now = self.clock.time.lock().map_err(|_| InterleavingError::Invariant)?;
                *now += chrono::Duration::seconds(1);
            }
            let heartbeat = PeerService::new(self.inner.clone(), self.policy.clone(), self.clock.clone())
                .heartbeat(
                    &self.owner,
                    proto::HeartbeatRequest {
                        attachment_ref: self.reference.clone().into(),
                        availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
                        ..Default::default()
                    },
                )
                .await;
            if heartbeat.is_err() {
                return Err(InterleavingError::Invariant);
            }
            {
                let mut now = self.clock.time.lock().map_err(|_| InterleavingError::Invariant)?;
                *now += chrono::Duration::seconds(1);
            }
        }
        self.inner
            .append_stream(request)
            .await
            .map_err(InterleavingError::Store)
    }

    fn classify_append_failure(&self, error: &Self::Error) -> AppendFailure {
        match error {
            InterleavingError::Store(error) => self.inner.classify_append_failure(error),
            InterleavingError::Invariant => AppendFailure::Fatal,
        }
    }
}

#[tokio::test]
async fn send_rebuilds_its_acceptance_time_after_a_validated_heartbeat_conflict() -> TestResult {
    let f = Fixture::new().await?;
    let initial = f.clock.now();
    let store = HeartbeatBeforeMessageAppend {
        inner: f.store.clone(),
        policy: f.policy.clone(),
        clock: f.clock.clone(),
        owner: f.alice.clone(),
        reference: f.a.clone(),
        remaining: Arc::new(AtomicUsize::new(1)),
    };
    let app = PeerApplication::new(
        store,
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let sent = app.send(&f.alice, f.send("time-race", "retry at the new time")).await?;
    let expected = initial + chrono::Duration::seconds(2);
    assert_eq!(
        sent.message.accepted_at.as_option(),
        Some(&trogonai_proto::convert::timestamp_from_datetime(&expected))
    );
    let peer = trogon_peer::PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let history = f
        .store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: trogon_decider_runtime::ReadFrom::Beginning,
        })
        .await?;
    let mut accepted = 0;
    for event in history.events {
        if matches!(
            event
                .decode::<proto::PeerEvent>()?
                .into_decoded()
                .ok_or(PeerPortError::Integrity)?
                .event,
            Some(trogonai_proto::peers::PeerEventCase::PeerMessageAccepted(_))
        ) {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 1);
    Ok(())
}

#[tokio::test]
async fn acknowledgment_rebuilds_its_time_after_a_validated_heartbeat_conflict() -> TestResult {
    let f = Fixture::new().await?;
    intake(&f, "ack-time-race", "acknowledge once").await?;
    let prepared = f.app().prepare_batch(&f.bob, prepare(&f)).await?;
    let batch_id = prepared.batch.batch_id.clone().ok_or(PeerPortError::Integrity)?;
    let initial = f.clock.now();
    let store = HeartbeatBeforeMessageAppend {
        inner: f.store.clone(),
        policy: f.policy.clone(),
        clock: f.clock.clone(),
        owner: f.bob.clone(),
        reference: f.b.clone(),
        remaining: Arc::new(AtomicUsize::new(1)),
    };
    let app = PeerApplication::new(
        store,
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let request = proto::AcknowledgeBatchRequest {
        attachment_ref: f.b.clone().into(),
        batch_id: Some(batch_id),
    };
    let result = app.acknowledge_batch(&f.bob, request.clone()).await?;
    let expected = initial + chrono::Duration::seconds(2);
    assert_eq!(
        result.batch.acknowledged_at.as_option(),
        Some(&trogonai_proto::convert::timestamp_from_datetime(&expected))
    );
    assert_eq!(result.batch.batch_id, prepared.batch.batch_id);
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert_eq!(app.acknowledge_batch(&f.bob, request).await?, result);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    Ok(())
}

#[tokio::test]
async fn preparation_rebuilds_its_cached_candidate_after_a_validated_heartbeat_conflict() -> TestResult {
    let f = Fixture::new().await?;
    intake(&f, "prepare-time-race", "prepare once").await?;
    let initial = f.clock.now();
    let store = HeartbeatBeforeMessageAppend {
        inner: f.store.clone(),
        policy: f.policy.clone(),
        clock: f.clock.clone(),
        owner: f.bob.clone(),
        reference: f.b.clone(),
        remaining: Arc::new(AtomicUsize::new(1)),
    };
    let app = PeerApplication::new(
        store,
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let result = app.prepare_batch(&f.bob, prepare(&f)).await?;
    let expected = initial + chrono::Duration::seconds(2);
    assert_eq!(
        result.batch.prepared_at.as_option(),
        Some(&trogonai_proto::convert::timestamp_from_datetime(&expected))
    );
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].text.as_deref(), Some("prepare once"));
    {
        let records = f.cache.records.lock().map_err(|_| PeerPortError::Unavailable)?;
        assert_eq!(records.len(), 2);
        let orphans = f.cache.orphans.lock().map_err(|_| PeerPortError::Unavailable)?;
        assert_eq!(orphans.len(), 1);
        for (key, record) in records.iter() {
            let expected_time = if orphans.contains(key) { initial } else { expected };
            assert_eq!(
                record.batch().prepared_at.as_option(),
                Some(&trogonai_proto::convert::timestamp_from_datetime(&expected_time))
            );
            assert_eq!(
                trogon_peer_context::PeerContext::render(record.batch(), record.messages())?.as_str(),
                record.frame()
            );
        }
    }
    assert_eq!(app.prepare_batch(&f.bob, prepare(&f)).await?, result);
    Ok(())
}

#[tokio::test]
async fn preparation_stops_after_one_retry_and_orphans_both_conflicting_candidates() -> TestResult {
    let f = Fixture::new().await?;
    intake(&f, "prepare-budget", "retain the pending intake").await?;
    let remaining = Arc::new(AtomicUsize::new(3));
    let app = PeerApplication::new(
        HeartbeatBeforeMessageAppend {
            inner: f.store.clone(),
            policy: f.policy.clone(),
            clock: f.clock.clone(),
            owner: f.bob.clone(),
            reference: f.b.clone(),
            remaining: remaining.clone(),
        },
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    assert!(matches!(
        app.prepare_batch(&f.bob, prepare(&f)).await,
        Err(trogon_peer_service::PeerApplicationError::Message(
            trogon_decider_runtime::CommandError::Append(InterleavingError::Store(source))
        )) if f.store.classify_append_failure(&source) == AppendFailure::WriteConflict
    ));
    assert_eq!(
        remaining.load(Ordering::SeqCst),
        1,
        "only two append attempts are allowed"
    );
    {
        let records = f.cache.records.lock().map_err(|_| PeerPortError::Integrity)?;
        let orphans = f.cache.orphans.lock().map_err(|_| PeerPortError::Integrity)?;
        assert_eq!(records.len(), 2);
        assert_eq!(orphans.len(), 2);
        assert!(records.keys().all(|key| orphans.contains(key)));
    }
    let recipient = trogon_peer::PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let history = f
        .store
        .read_stream(ReadStreamRequest {
            stream_id: &recipient,
            from: trogon_decider_runtime::ReadFrom::Beginning,
        })
        .await?;
    assert_eq!(
        history.events.len(),
        5,
        "only enrollment, intake and two heartbeats may commit"
    );
    for event in history.events.into_iter().skip(3) {
        assert!(matches!(
            event
                .decode::<proto::PeerEvent>()?
                .into_decoded()
                .ok_or(PeerPortError::Integrity)?
                .event,
            Some(trogonai_proto::peers::PeerEventCase::PeerHeartbeatRecorded(_))
        ));
    }
    Ok(())
}

#[tokio::test]
async fn reconciliation_revalidates_and_retries_a_recipient_heartbeat_conflict() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f.app().send(&f.alice, f.send("intake-time-race", "one intake")).await?;
    let sender = trogon_peer::PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let message = trogon_peer::MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let app = PeerApplication::new(
        HeartbeatBeforeMessageAppend {
            inner: f.store.clone(),
            policy: f.policy.clone(),
            clock: f.clock.clone(),
            owner: f.bob.clone(),
            reference: f.b.clone(),
            remaining: Arc::new(AtomicUsize::new(1)),
        },
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let limit = trogon_peer_service::WorkLimit::new(1)?;
    assert_eq!(
        app.reconcile(&f.alice, &sender, std::slice::from_ref(&message), limit)
            .await?,
        1
    );
    let recipient = trogon_peer::PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let history = f
        .store
        .read_stream(ReadStreamRequest {
            stream_id: &recipient,
            from: trogon_decider_runtime::ReadFrom::Beginning,
        })
        .await?;
    assert_eq!(
        history.events.len(),
        4,
        "only enrollment, heartbeat and one intake may commit"
    );
    assert!(matches!(
        history.events[3]
            .decode::<proto::PeerEvent>()?
            .into_decoded()
            .ok_or(PeerPortError::Integrity)?
            .event,
        Some(trogonai_proto::peers::PeerEventCase::PeerMessageIntakeRecorded(_))
    ));
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert_eq!(app.reconcile(&f.alice, &sender, &[message], limit).await?, 1);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    Ok(())
}

fn prepare(f: &Fixture) -> proto::PrepareBatchRequest {
    proto::PrepareBatchRequest {
        attachment_ref: f.b.clone().into(),
        prompt_id: Some("prompt".into()),
    }
}

async fn intake(f: &Fixture, key: &str, text: &str) -> TestResult {
    let sent = f.app().send(&f.alice, f.send(key, text)).await?;
    let id = trogon_peer::MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let sender = trogon_peer::PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    f.app()
        .reconcile(&f.alice, &sender, &[id], trogon_peer_service::WorkLimit::new(1)?)
        .await?;
    Ok(())
}
