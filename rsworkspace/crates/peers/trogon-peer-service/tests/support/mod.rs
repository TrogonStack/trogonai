use chrono::{DateTime, TimeZone, Utc};
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use trogon_decider_runtime::*;
use trogon_peer::*;
use trogon_peer_service::*;
use trogonai_proto::peers::v1alpha1 as proto;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Clone)]
pub struct StoreFixture {
    pub inner: InMemoryStore,
    pub tenant: TenantId,
    pub reads: Arc<AtomicUsize>,
    pub appends: Arc<AtomicUsize>,
    pub append_barrier: Arc<Mutex<Option<Arc<tokio::sync::Barrier>>>>,
    pub lose_append_reply: Arc<AtomicBool>,
}
impl StoreFixture {
    pub fn new(tenant: TenantId) -> Self {
        Self {
            inner: InMemoryStore::new(),
            tenant,
            reads: Arc::default(),
            appends: Arc::default(),
            append_barrier: Arc::default(),
            lose_append_reply: Arc::default(),
        }
    }
}
impl ScopedPeerStore for StoreFixture {
    fn tenant(&self) -> &TenantId {
        &self.tenant
    }
}
impl StreamRead<PeerId> for StoreFixture {
    type Error = <InMemoryStore as StreamRead<str>>::Error;
    async fn read_stream(&self, r: ReadStreamRequest<'_, PeerId>) -> Result<ReadStreamResponse, Self::Error> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let key = format!("{}:{}", self.tenant.as_str(), r.stream_id.as_str());
        self.inner
            .read_stream(ReadStreamRequest {
                stream_id: key.as_str(),
                from: r.from,
            })
            .await
    }
    async fn read_stream_bounded(
        &self,
        r: ReadStreamRequest<'_, PeerId>,
        max: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let key = format!("{}:{}", self.tenant.as_str(), r.stream_id.as_str());
        self.inner
            .read_stream_bounded(
                ReadStreamRequest {
                    stream_id: key.as_str(),
                    from: r.from,
                },
                max,
            )
            .await
    }
}
impl StreamAppend<PeerId> for StoreFixture {
    type Error = AppendFixtureError;
    async fn append_stream(&self, r: AppendStreamRequest<'_, PeerId>) -> Result<AppendStreamResponse, Self::Error> {
        let barrier = self.append_barrier.lock().map(|v| v.clone()).unwrap_or_default();
        if let Some(barrier) = barrier {
            barrier.wait().await;
        }
        self.appends.fetch_add(1, Ordering::SeqCst);
        let key = format!("{}:{}", self.tenant.as_str(), r.stream_id.as_str());
        let response = self
            .inner
            .append_stream(AppendStreamRequest {
                stream_id: key.as_str(),
                stream_write_precondition: r.stream_write_precondition,
                events: r.events,
            })
            .await
            .map_err(AppendFixtureError::Backend)?;
        if self.lose_append_reply.swap(false, Ordering::SeqCst) {
            return Err(AppendFixtureError::LostResponse);
        }
        Ok(response)
    }
    fn classify_append_failure(&self, e: &Self::Error) -> AppendFailure {
        match e {
            AppendFixtureError::Backend(e) => {
                <InMemoryStore as StreamAppend<str>>::classify_append_failure(&self.inner, e)
            }
            AppendFixtureError::LostResponse => AppendFailure::Fatal,
        }
    }
}
#[derive(Clone)]
pub struct ClockFixture {
    pub time: Arc<Mutex<DateTime<Utc>>>,
}
impl PeerClock for ClockFixture {
    fn now(&self) -> DateTime<Utc> {
        self.time.lock().map(|v| *v).unwrap_or(DateTime::<Utc>::MIN_UTC)
    }
}
#[derive(Clone)]
pub struct PolicyFixture {
    pub tenant: TenantId,
    pub deny: Arc<AtomicBool>,
    pub grants: Arc<Mutex<BTreeMap<String, TrustedPeerOwner>>>,
}
impl PolicyFixture {
    pub fn owner(
        &self,
        principal: &CommandPrincipal,
        owner: &TrustedPeerOwner,
    ) -> Result<(), AuthorizationDeniedError> {
        if self.deny.load(Ordering::SeqCst)
            || owner.tenant() != &self.tenant
            || principal.kind() != PrincipalKind::Agent
            || principal.id().as_str() != owner.caller().as_str()
        {
            return Err(AuthorizationDeniedError::new("outside fixture grant"));
        }
        Ok(())
    }
    fn peer(
        &self,
        principal: &CommandPrincipal,
        owner: &TrustedPeerOwner,
        peer: &PeerId,
    ) -> Result<(), AuthorizationDeniedError> {
        self.owner(principal, owner)?;
        if self
            .grants
            .lock()
            .map_err(|_| AuthorizationDeniedError::new("grant unavailable"))?
            .get(peer.as_str())
            != Some(owner)
        {
            return Err(AuthorizationDeniedError::new("peer not owned"));
        }
        Ok(())
    }
}
impl CommandAuthorizer<Enroll> for PolicyFixture {
    fn authorize(&self, p: &CommandPrincipal, c: &Enroll) -> Result<(), AuthorizationDeniedError> {
        self.owner(p, c.identity().owner())?;
        if c.collaboration().as_str() != "shared" {
            return Err(AuthorizationDeniedError::new("collaboration denied"));
        }
        Ok(())
    }
}
impl CommandAuthorizer<Attach> for PolicyFixture {
    fn authorize(&self, p: &CommandPrincipal, c: &Attach) -> Result<(), AuthorizationDeniedError> {
        self.owner(p, c.identity().owner())?;
        if c.collaboration().as_str() != "shared" {
            return Err(AuthorizationDeniedError::new("collaboration denied"));
        }
        Ok(())
    }
}
impl CommandAuthorizer<PeerAccess> for PolicyFixture {
    fn authorize(&self, p: &CommandPrincipal, c: &PeerAccess) -> Result<(), AuthorizationDeniedError> {
        self.owner(p, c.owner())?;
        let grants = self
            .grants
            .lock()
            .map_err(|_| AuthorizationDeniedError::new("grant unavailable"))?;
        if c.operation() == PeerOperation::ReadPeer {
            if grants.contains_key(c.peer().as_str()) {
                return Ok(());
            }
        } else if grants.get(c.peer().as_str()) == Some(c.owner()) {
            return Ok(());
        }
        Err(AuthorizationDeniedError::new("peer outside grant"))
    }
}
macro_rules! owner_policy { ($($cmd:ty),*) => { $(impl CommandAuthorizer<$cmd> for PolicyFixture { fn authorize(&self, p: &CommandPrincipal, c: &$cmd) -> Result<(), AuthorizationDeniedError> { self.peer(p, c.owner(), c.attachment().peer_id()) } })* }; }
owner_policy!(Heartbeat, Detach);
impl CommandAuthorizer<IntakeMessage> for PolicyFixture {
    fn authorize(&self, p: &CommandPrincipal, c: &IntakeMessage) -> Result<(), AuthorizationDeniedError> {
        self.evidence(p, c.evidence().tenant(), c.evidence().metadata())
    }
}
impl CommandAuthorizer<DisposeMessage> for PolicyFixture {
    fn authorize(&self, p: &CommandPrincipal, c: &DisposeMessage) -> Result<(), AuthorizationDeniedError> {
        match c.disposition() {
            TerminalDisposition::Expired | TerminalDisposition::Unavailable => {
                self.evidence(p, c.evidence().tenant(), c.evidence().metadata())
            }
        }
    }
}
impl PolicyFixture {
    fn evidence(
        &self,
        p: &CommandPrincipal,
        tenant: &TenantId,
        m: &proto::MessageMetadata,
    ) -> Result<(), AuthorizationDeniedError> {
        if self.deny.load(Ordering::SeqCst)
            || tenant != &self.tenant
            || p.kind() != PrincipalKind::Agent
            || m.collaboration_ref.as_deref() != Some("shared")
        {
            return Err(AuthorizationDeniedError::new("evidence outside grant"));
        }
        let grants = self
            .grants
            .lock()
            .map_err(|_| AuthorizationDeniedError::new("grant unavailable"))?;
        if !grants.contains_key(m.recipient_peer_id.as_deref().unwrap_or_default())
            || !grants.contains_key(m.sender_peer_id.as_deref().unwrap_or_default())
        {
            return Err(AuthorizationDeniedError::new("evidence peers denied"));
        }
        Ok(())
    }
}
impl CommandAuthorizer<SettleMessage> for PolicyFixture {
    fn authorize(&self, p: &CommandPrincipal, c: &SettleMessage) -> Result<(), AuthorizationDeniedError> {
        if self.deny.load(Ordering::SeqCst)
            || c.evidence().tenant() != &self.tenant
            || p.kind() != PrincipalKind::Agent
            || !self
                .grants
                .lock()
                .map_err(|_| AuthorizationDeniedError::new("grant unavailable"))?
                .contains_key(c.sender().as_str())
        {
            return Err(AuthorizationDeniedError::new("settlement denied"));
        }
        Ok(())
    }
}

/// Disposable plaintext fixture only. This is not ADR 0083 sealing or a production fingerprint implementation.
#[derive(Clone, Default)]
pub struct DisposableContentFixture {
    pub calls: Arc<AtomicUsize>,
    pub bodies: Arc<Mutex<BTreeMap<String, MessageText>>>,
    pub fingerprints: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    pub fail_open: Arc<AtomicBool>,
}
impl ContentSeal for DisposableContentFixture {
    async fn seal(&self, scope: &PeerAccess, text: &MessageText) -> Result<ContentBinding, PeerPortError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let reference = format!("disposable-{n}");
        let mut fps = self.fingerprints.lock().map_err(|_| PeerPortError::Unavailable)?;
        let token = vec![(fps.len() + 1) as u8; 32];
        let fingerprint = fps.entry(text.as_str().to_owned()).or_insert(token).clone();
        self.bodies
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .insert(reference.clone(), text.clone());
        ContentBinding::from_port(
            scope.owner().tenant().clone(),
            SealedContentRef::new(proto::ClaimCheck {
                storage_ref: proto::StorageLocator { value: Some(reference) }.into(),
                digest: proto::Digest {
                    algorithm: Some("sha256".into()),
                    value: Some(<sha2::Sha256 as sha2::Digest>::digest(text.as_str().as_bytes()).to_vec()),
                }
                .into(),
            })
            .map_err(|_| PeerPortError::Integrity)?,
            text.as_str().len() as u32,
            fingerprint,
        )
        .map_err(|_| PeerPortError::Integrity)
    }
}
impl ContentOpen for DisposableContentFixture {
    async fn open(&self, scope: &PeerAccess, binding: &ContentBinding) -> Result<Option<MessageText>, PeerPortError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_open.load(Ordering::SeqCst) {
            return Err(PeerPortError::Unavailable);
        }
        if binding.tenant() != scope.owner().tenant() {
            return Err(PeerPortError::Denied);
        }
        let text = self
            .bodies
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .get(binding.content().reference().as_str())
            .cloned();
        if let Some(text) = &text {
            let fingerprints = self.fingerprints.lock().map_err(|_| PeerPortError::Unavailable)?;
            let integrity = <sha2::Sha256 as sha2::Digest>::digest(text.as_str().as_bytes());
            if &integrity[..] != binding.content().reference().integrity_digest()
                || text.as_str().len() != binding.content().size() as usize
                || fingerprints.get(text.as_str()).map(Vec::as_slice) != Some(binding.content().semantic_fingerprint())
            {
                return Err(PeerPortError::Integrity);
            }
        }
        Ok(text)
    }
}
#[derive(Clone, Default)]
pub struct PresentationFixture {
    pub records: Arc<Mutex<BTreeMap<String, PreparedPresentation>>>,
    pub orphans: Arc<Mutex<Vec<String>>>,
    pub calls: Arc<AtomicUsize>,
    pub fail_insert: Arc<AtomicBool>,
}
impl PresentationFixture {
    pub fn key(k: &PresentationKey) -> String {
        format!("{}:{}:{}", k.tenant().as_str(), k.peer().as_str(), k.batch().as_str())
    }
}
impl PresentationRead for PresentationFixture {
    async fn read(&self, k: &PresentationKey) -> Result<Option<PreparedPresentation>, PeerPortError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .records
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .get(&Self::key(k))
            .cloned())
    }
}
impl PresentationInsert for PresentationFixture {
    async fn insert(&self, k: &PresentationKey, r: &PreparedPresentation) -> Result<(), PeerPortError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_insert.load(Ordering::SeqCst) {
            return Err(PeerPortError::Unavailable);
        }
        let mut records = self.records.lock().map_err(|_| PeerPortError::Unavailable)?;
        match records.get(&Self::key(k)) {
            Some(old) if old != r => Err(PeerPortError::Conflict),
            Some(_) => Ok(()),
            None => {
                records.insert(Self::key(k), r.clone());
                Ok(())
            }
        }
    }
}
impl PresentationOrphan for PresentationFixture {
    async fn orphan(&self, k: &PresentationKey) -> Result<(), PeerPortError> {
        self.orphans
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .push(Self::key(k));
        Ok(())
    }
}
#[derive(Clone, Default)]
pub struct IdFixture(pub Arc<AtomicUsize>);
impl MessageIdGenerator for IdFixture {
    fn message_id(&self) -> Result<MessageId, PeerPortError> {
        MessageId::new(format!("message-{}", self.0.fetch_add(1, Ordering::SeqCst)))
            .map_err(|_| PeerPortError::Integrity)
    }
}
impl BatchIdGenerator for IdFixture {
    fn batch_id(&self) -> Result<BatchId, PeerPortError> {
        BatchId::new(format!("batch-{}", self.0.fetch_add(1, Ordering::SeqCst))).map_err(|_| PeerPortError::Integrity)
    }
}
#[derive(Clone, Default)]
pub struct CatalogFixture {
    pub peers: Arc<Mutex<Vec<PeerId>>>,
    pub calls: Arc<AtomicUsize>,
}
impl PeerCatalog for CatalogFixture {
    async fn page(
        &self,
        _scope: &PeerAccess,
        _collaboration: &CollaborationRef,
        after: Option<&PeerId>,
        limit: WorkLimit,
    ) -> Result<Vec<PeerId>, PeerPortError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut peers = self.peers.lock().map_err(|_| PeerPortError::Unavailable)?.clone();
        peers.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        Ok(peers
            .into_iter()
            .filter(|id| after.is_none_or(|a| id.as_str() > a.as_str()))
            .take(limit.get())
            .collect())
    }
}
pub type AppFixture = PeerApplication<
    StoreFixture,
    PolicyFixture,
    ClockFixture,
    DisposableContentFixture,
    PresentationFixture,
    IdFixture,
    CatalogFixture,
>;
pub struct Fixture {
    pub store: StoreFixture,
    pub policy: PolicyFixture,
    pub clock: ClockFixture,
    pub content: DisposableContentFixture,
    pub cache: PresentationFixture,
    pub ids: IdFixture,
    pub catalog: CatalogFixture,
    pub alice: TrustedPeerOwner,
    pub bob: TrustedPeerOwner,
    pub a: proto::AttachmentRef,
    pub b: proto::AttachmentRef,
}
impl Fixture {
    pub async fn new() -> TestResult<Self> {
        let tenant = TenantId::new("tenant")?;
        let alice = TrustedPeerOwner::new(tenant.clone(), CallerId::new("alice")?);
        let bob = TrustedPeerOwner::new(tenant.clone(), CallerId::new("bob")?);
        let store = StoreFixture::new(tenant.clone());
        let policy = PolicyFixture {
            tenant,
            deny: Arc::default(),
            grants: Arc::default(),
        };
        let clock = ClockFixture {
            time: Arc::new(Mutex::new(
                Utc.with_ymd_and_hms(2026, 11, 3, 12, 0, 0)
                    .single()
                    .ok_or(PeerPortError::Integrity)?,
            )),
        };
        let service = PeerService::new(store.clone(), policy.clone(), clock.clone());
        let a = service
            .enroll(&alice, Self::enrollment("alice"))
            .await?
            .attachment
            .attachment_ref
            .clone()
            .into_option()
            .ok_or(PeerPortError::Integrity)?;
        let b = service
            .enroll(&bob, Self::enrollment("bob"))
            .await?
            .attachment
            .attachment_ref
            .clone()
            .into_option()
            .ok_or(PeerPortError::Integrity)?;
        let aid = PeerId::parse(a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
        let bid = PeerId::parse(b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
        policy
            .grants
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .extend([(aid.to_string(), alice.clone()), (bid.to_string(), bob.clone())]);
        let catalog = CatalogFixture::default();
        catalog
            .peers
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .extend([aid, bid]);
        Ok(Self {
            store,
            policy,
            clock,
            content: DisposableContentFixture::default(),
            cache: PresentationFixture::default(),
            ids: IdFixture::default(),
            catalog,
            alice,
            bob,
            a,
            b,
        })
    }
    pub fn enrollment(name: &str) -> proto::EnrollRequest {
        proto::EnrollRequest {
            installation_id: Some("install".into()),
            native_session_id: Some(name.into()),
            connector_id: Some(name.into()),
            collaboration_ref: Some("shared".into()),
            display_name: Some(name.into()),
        }
    }
    pub fn app(&self) -> AppFixture {
        PeerApplication::new(
            self.store.clone(),
            self.policy.clone(),
            self.clock.clone(),
            self.content.clone(),
            self.cache.clone(),
            self.ids.clone(),
            self.catalog.clone(),
        )
    }
    pub fn send(&self, key: &str, text: &str) -> proto::SendRequest {
        proto::SendRequest {
            attachment_ref: self.a.clone().into(),
            recipient_peer_id: self.b.peer_id.clone(),
            idempotency_key: Some(key.into()),
            text: Some(text.into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppendFixtureError {
    #[error("fixture store failure: {0}")]
    Backend(#[source] StreamAppendError),
    #[error("fixture lost append response")]
    LostResponse,
}

impl CommandAuthorizer<SendMessage> for PolicyFixture {
    fn authorize(&self, p: &CommandPrincipal, c: &SendMessage) -> Result<(), AuthorizationDeniedError> {
        self.peer(p, c.owner(), c.attachment().peer_id())?;
        if c.collaboration().as_str() != "shared"
            || !c.content().reference().as_str().starts_with("disposable-")
            || c.content().size() > 2048
            || c.content().semantic_fingerprint().len() != 32
            || !self
                .grants
                .lock()
                .map_err(|_| AuthorizationDeniedError::new("grant unavailable"))?
                .contains_key(c.recipient().as_str())
        {
            return Err(AuthorizationDeniedError::new("message metadata outside fixture grant"));
        }
        Ok(())
    }
}
impl CommandAuthorizer<PrepareBatch> for PolicyFixture {
    fn authorize(&self, p: &CommandPrincipal, c: &PrepareBatch) -> Result<(), AuthorizationDeniedError> {
        self.peer(p, c.owner(), c.attachment().peer_id())?;
        if c.batch_id().as_str() == "denied"
            || c.plan().message_ids().is_empty()
            || c.plan().rendered_utf8_bytes() > 8192
        {
            return Err(AuthorizationDeniedError::new("plan outside fixture grant"));
        }
        Ok(())
    }
}
impl CommandAuthorizer<AcknowledgeBatch> for PolicyFixture {
    fn authorize(&self, p: &CommandPrincipal, c: &AcknowledgeBatch) -> Result<(), AuthorizationDeniedError> {
        self.peer(p, c.owner(), c.attachment().peer_id())?;
        if c.batch_id().as_str() == "denied" {
            return Err(AuthorizationDeniedError::new("batch outside fixture grant"));
        }
        Ok(())
    }
}
