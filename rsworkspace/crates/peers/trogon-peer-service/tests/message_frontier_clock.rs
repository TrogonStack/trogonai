use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, AppendStreamResponse, AuthorizationDeniedError, CommandAuthorizer,
    CommandPrincipal, ReadStreamRequest, ReadStreamResponse, StreamAppend, StreamRead,
};
use trogon_peer::{PeerId, SendMessage};
use trogon_peer_service::{PeerAccess, PeerApplication, PeerClock, PeerPortError, PeerService};
use trogonai_proto::peers::v1alpha1 as proto;
mod support;
use support::*;

struct ArmHeartbeat {
    inner: PolicyFixture,
    authorizations: AtomicUsize,
    inject_at: usize,
    armed: Arc<AtomicBool>,
}
impl CommandAuthorizer<PeerAccess> for ArmHeartbeat {
    fn authorize(&self, principal: &CommandPrincipal, command: &PeerAccess) -> Result<(), AuthorizationDeniedError> {
        self.inner.authorize(principal, command)
    }
}
impl CommandAuthorizer<SendMessage> for ArmHeartbeat {
    fn authorize(&self, principal: &CommandPrincipal, command: &SendMessage) -> Result<(), AuthorizationDeniedError> {
        self.inner.authorize(principal, command)?;
        if self.authorizations.fetch_add(1, Ordering::SeqCst) + 1 == self.inject_at {
            self.armed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
enum InterleavingError {
    #[error("heartbeat interleaving failed")]
    Invariant,
    #[error(transparent)]
    Read(<StoreFixture as StreamRead<PeerId>>::Error),
    #[error(transparent)]
    Append(<StoreFixture as StreamAppend<PeerId>>::Error),
}

struct HeartbeatBeforeHistoryRead {
    inner: StoreFixture,
    policy: PolicyFixture,
    clock: ClockFixture,
    owner: trogon_peer::TrustedPeerOwner,
    reference: proto::AttachmentRef,
    armed: Arc<AtomicBool>,
    injected: Arc<AtomicUsize>,
}
impl HeartbeatBeforeHistoryRead {
    async fn interleave(&self) -> Result<(), InterleavingError> {
        if self.armed.swap(false, Ordering::SeqCst) {
            {
                let mut time = self.clock.time.lock().map_err(|_| InterleavingError::Invariant)?;
                *time += chrono::Duration::seconds(1);
            }
            PeerService::new(self.inner.clone(), self.policy.clone(), self.clock.clone())
                .heartbeat(
                    &self.owner,
                    proto::HeartbeatRequest {
                        attachment_ref: self.reference.clone().into(),
                        availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| InterleavingError::Invariant)?;
            {
                let mut time = self.clock.time.lock().map_err(|_| InterleavingError::Invariant)?;
                *time += chrono::Duration::seconds(1);
            }
            self.injected.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}
impl trogon_peer_service::ScopedPeerStore for HeartbeatBeforeHistoryRead {
    fn tenant(&self) -> &trogon_peer::TenantId {
        &self.inner.tenant
    }
}
impl StreamRead<PeerId> for HeartbeatBeforeHistoryRead {
    type Error = InterleavingError;
    async fn read_stream(&self, request: ReadStreamRequest<'_, PeerId>) -> Result<ReadStreamResponse, Self::Error> {
        self.interleave().await?;
        self.inner.read_stream(request).await.map_err(InterleavingError::Read)
    }
    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, PeerId>,
        maximum: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.interleave().await?;
        self.inner
            .read_stream_bounded(request, maximum)
            .await
            .map_err(InterleavingError::Read)
    }
}
impl StreamAppend<PeerId> for HeartbeatBeforeHistoryRead {
    type Error = InterleavingError;
    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, PeerId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        self.inner
            .append_stream(request)
            .await
            .map_err(InterleavingError::Append)
    }
    fn classify_append_failure(&self, error: &Self::Error) -> AppendFailure {
        match error {
            InterleavingError::Append(error) => self.inner.classify_append_failure(error),
            InterleavingError::Invariant | InterleavingError::Read(_) => AppendFailure::Fatal,
        }
    }
}

async fn send_after_a_history_heartbeat(inject_at: usize) -> TestResult {
    let f = Fixture::new().await?;
    assert!(
        f.app()
            .send(&f.bob, f.send("wrong-owner", "reject before storage"))
            .await
            .is_err()
    );
    let initial = f.clock.now();
    let armed = Arc::new(AtomicBool::new(false));
    let injected = Arc::new(AtomicUsize::new(0));
    let app = PeerApplication::new(
        HeartbeatBeforeHistoryRead {
            inner: f.store.clone(),
            policy: f.policy.clone(),
            clock: f.clock.clone(),
            owner: f.alice.clone(),
            reference: f.a.clone(),
            armed: armed.clone(),
            injected: injected.clone(),
        },
        ArmHeartbeat {
            inner: f.policy.clone(),
            authorizations: AtomicUsize::new(0),
            inject_at,
            armed,
        },
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let sent = app.send(&f.alice, f.send("frontier-clock", "one acceptance")).await;
    assert_eq!(
        injected.load(Ordering::SeqCst),
        1,
        "the competing heartbeat must commit"
    );
    let sent = sent?;
    let expected = initial + chrono::Duration::seconds(2);
    assert_eq!(
        sent.message.accepted_at.as_option(),
        Some(&trogonai_proto::convert::timestamp_from_datetime(&expected))
    );
    let sender = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let history = f
        .store
        .read_stream(ReadStreamRequest {
            stream_id: &sender,
            from: trogon_decider_runtime::ReadFrom::Beginning,
        })
        .await?;
    assert_eq!(
        history.events.len(),
        4,
        "enrollment, competing heartbeat and exactly one acceptance"
    );
    Ok(())
}

#[tokio::test]
async fn send_samples_time_after_the_strict_history_frontier() -> TestResult {
    send_after_a_history_heartbeat(1).await
}

#[tokio::test]
async fn send_recovers_a_clock_regression_only_after_the_validated_frontier_advances() -> TestResult {
    send_after_a_history_heartbeat(2).await
}

struct HeartbeatDuringCacheInsert {
    inner: PresentationFixture,
    store: StoreFixture,
    policy: PolicyFixture,
    clock: ClockFixture,
    owner: trogon_peer::TrustedPeerOwner,
    reference: proto::AttachmentRef,
    injected: Arc<AtomicBool>,
}
impl trogon_peer_service::PresentationRead for HeartbeatDuringCacheInsert {
    async fn read(
        &self,
        key: &trogon_peer_service::PresentationKey,
    ) -> Result<Option<trogon_peer_service::PreparedPresentation>, PeerPortError> {
        trogon_peer_service::PresentationRead::read(&self.inner, key).await
    }
}
impl trogon_peer_service::PresentationOrphan for HeartbeatDuringCacheInsert {
    async fn orphan(&self, key: &trogon_peer_service::PresentationKey) -> Result<(), PeerPortError> {
        trogon_peer_service::PresentationOrphan::orphan(&self.inner, key).await
    }
}
impl trogon_peer_service::PresentationInsert for HeartbeatDuringCacheInsert {
    async fn insert(
        &self,
        key: &trogon_peer_service::PresentationKey,
        record: &trogon_peer_service::PreparedPresentation,
    ) -> Result<(), PeerPortError> {
        trogon_peer_service::PresentationInsert::insert(&self.inner, key, record).await?;
        if !self.injected.swap(true, Ordering::SeqCst) {
            {
                let mut time = self.clock.time.lock().map_err(|_| PeerPortError::Integrity)?;
                *time += chrono::Duration::seconds(1);
            }
            PeerService::new(self.store.clone(), self.policy.clone(), self.clock.clone())
                .heartbeat(
                    &self.owner,
                    proto::HeartbeatRequest {
                        attachment_ref: self.reference.clone().into(),
                        availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| PeerPortError::Integrity)?;
            {
                let mut time = self.clock.time.lock().map_err(|_| PeerPortError::Integrity)?;
                *time += chrono::Duration::seconds(1);
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn preparation_rebuilds_an_immutable_candidate_when_a_heartbeat_commits_during_cache_insert() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f
        .app()
        .send(&f.alice, f.send("cache-heartbeat", "immutable context"))
        .await?;
    let sender = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let message = trogon_peer::MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    f.app()
        .reconcile(&f.alice, &sender, &[message], trogon_peer_service::WorkLimit::new(1)?)
        .await?;
    let initial = f.clock.now();
    let injected = Arc::new(AtomicBool::new(false));
    let app = PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        HeartbeatDuringCacheInsert {
            inner: f.cache.clone(),
            store: f.store.clone(),
            policy: f.policy.clone(),
            clock: f.clock.clone(),
            owner: f.bob.clone(),
            reference: f.b.clone(),
            injected: injected.clone(),
        },
        f.ids.clone(),
        f.catalog.clone(),
    );
    let request = proto::PrepareBatchRequest {
        attachment_ref: f.b.clone().into(),
        prompt_id: Some("prompt".into()),
    };
    let prepared = app.prepare_batch(&f.bob, request.clone()).await;
    assert!(
        injected.load(Ordering::SeqCst),
        "cache insertion must advance the attachment frontier"
    );
    let prepared = prepared?;
    let expected = initial + chrono::Duration::seconds(2);
    assert_eq!(
        prepared.batch.prepared_at.as_option(),
        Some(&trogonai_proto::convert::timestamp_from_datetime(&expected))
    );
    assert_eq!(prepared.messages[0].text.as_deref(), Some("immutable context"));
    {
        let records = f.cache.records.lock().map_err(|_| PeerPortError::Integrity)?;
        let orphans = f.cache.orphans.lock().map_err(|_| PeerPortError::Integrity)?;
        assert_eq!(records.len(), 2);
        assert_eq!(orphans.len(), 1);
        for (key, record) in records.iter() {
            let time = if orphans.contains(key) { initial } else { expected };
            assert_eq!(
                record.batch().prepared_at.as_option(),
                Some(&trogonai_proto::convert::timestamp_from_datetime(&time))
            );
            assert_eq!(
                trogon_peer_context::PeerContext::render(record.batch(), record.messages())?.as_str(),
                record.frame()
            );
        }
    }
    assert_eq!(app.prepare_batch(&f.bob, request).await?, prepared);
    Ok(())
}

struct BackwardClock {
    steady: chrono::DateTime<chrono::Utc>,
    regressed: chrono::DateTime<chrono::Utc>,
    regress_on: usize,
    reads: Arc<AtomicUsize>,
}
struct AuthorizedBackwardClock {
    steady: chrono::DateTime<chrono::Utc>,
    regressed: chrono::DateTime<chrono::Utc>,
    armed: Arc<AtomicBool>,
}

impl PeerClock for AuthorizedBackwardClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        if self.armed.load(Ordering::SeqCst) {
            self.regressed
        } else {
            self.steady
        }
    }
}

struct RegressOnSendAuthorization {
    inner: PolicyFixture,
    armed: Arc<AtomicBool>,
    authorizations: Arc<AtomicUsize>,
}

impl CommandAuthorizer<PeerAccess> for RegressOnSendAuthorization {
    fn authorize(&self, principal: &CommandPrincipal, command: &PeerAccess) -> Result<(), AuthorizationDeniedError> {
        self.inner.authorize(principal, command)
    }
}

impl CommandAuthorizer<SendMessage> for RegressOnSendAuthorization {
    fn authorize(&self, principal: &CommandPrincipal, command: &SendMessage) -> Result<(), AuthorizationDeniedError> {
        self.inner.authorize(principal, command)?;
        self.authorizations.fetch_add(1, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl PeerClock for BackwardClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        if self.reads.fetch_add(1, Ordering::SeqCst) >= self.regress_on {
            self.regressed
        } else {
            self.steady
        }
    }
}

async fn renew_at_next_second(
    f: &Fixture,
    owner: &trogon_peer::TrustedPeerOwner,
    reference: &proto::AttachmentRef,
) -> TestResult<chrono::DateTime<chrono::Utc>> {
    let previous = f.clock.now();
    *f.clock.time.lock().map_err(|_| PeerPortError::Integrity)? = previous + chrono::Duration::seconds(1);
    PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone())
        .heartbeat(
            owner,
            proto::HeartbeatRequest {
                attachment_ref: reference.clone().into(),
                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
                ..Default::default()
            },
        )
        .await?;
    Ok(previous)
}

#[tokio::test]
async fn send_does_not_retry_a_backward_clock_without_a_new_history_revision() -> TestResult {
    let f = Fixture::new().await?;
    let previous = renew_at_next_second(&f, &f.alice, &f.a).await?;
    let armed = Arc::new(AtomicBool::new(false));
    let authorizations = Arc::new(AtomicUsize::new(0));
    let app = PeerApplication::new(
        f.store.clone(),
        RegressOnSendAuthorization {
            inner: f.policy.clone(),
            armed: armed.clone(),
            authorizations: authorizations.clone(),
        },
        AuthorizedBackwardClock {
            steady: f.clock.now(),
            regressed: previous,
            armed,
        },
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        app.send(&f.alice, f.send("backward-clock", "must not append")).await,
        Err(trogon_peer_service::PeerApplicationError::Message(
            trogon_decider_runtime::CommandError::Decide(trogon_peer::MessageError::Guard(
                trogon_peer::HeartbeatError::ClockRegression
            ))
        ))
    ));
    assert_eq!(
        authorizations.load(Ordering::SeqCst),
        2,
        "a genuine clock regression must not spend a conflict retry"
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    Ok(())
}

#[tokio::test]
async fn preparation_does_not_retry_a_backward_clock_without_a_new_history_revision() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f
        .app()
        .send(&f.alice, f.send("backward-prepare", "eligible context"))
        .await?;
    let sender = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let message = trogon_peer::MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    f.app()
        .reconcile(&f.alice, &sender, &[message], trogon_peer_service::WorkLimit::new(1)?)
        .await?;
    let previous = renew_at_next_second(&f, &f.bob, &f.b).await?;
    let reads = Arc::new(AtomicUsize::new(0));
    let app = PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        BackwardClock {
            steady: f.clock.now(),
            regressed: previous,
            regress_on: 1,
            reads: reads.clone(),
        },
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        app.prepare_batch(
            &f.bob,
            proto::PrepareBatchRequest {
                attachment_ref: f.b.clone().into(),
                prompt_id: Some("prompt".into())
            }
        )
        .await,
        Err(trogon_peer_service::PeerApplicationError::Message(
            trogon_decider_runtime::CommandError::Decide(trogon_peer::MessageError::Guard(
                trogon_peer::HeartbeatError::ClockRegression
            ))
        ))
    ));
    assert_eq!(
        reads.load(Ordering::SeqCst),
        2,
        "a genuine clock regression must not rebuild another candidate"
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.cache.records.lock().map_err(|_| PeerPortError::Integrity)?.len(), 1);
    assert_eq!(f.cache.orphans.lock().map_err(|_| PeerPortError::Integrity)?.len(), 1);
    Ok(())
}
