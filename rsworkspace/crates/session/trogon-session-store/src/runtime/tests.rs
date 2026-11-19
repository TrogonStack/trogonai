#![allow(clippy::panic, clippy::unwrap_used)]

use std::{
    convert::Infallible,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;
use crate::{
    SessionAdmissionLease,
    admission::{ChildAdmission, CreateAdmission, DispatchAdmission},
};
use trogon_decider_runtime::{AppendStreamResponse, EventId, ReadStreamRequest, ReadStreamResponse};

#[derive(Default)]
struct NoEffects(AtomicUsize);

impl StreamRead<SessionKey> for NoEffects {
    type Error = Infallible;
    async fn read_stream(
        &self,
        _request: ReadStreamRequest<'_, SessionKey>,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("oversized original must not cause a source read")
    }
}

impl StreamAppend<SessionKey> for NoEffects {
    type Error = Infallible;
    async fn append_stream(
        &self,
        _request: AppendStreamRequest<'_, SessionKey>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("oversized original must not reach persistence")
    }
}

struct UnusedLease;
impl SessionAdmissionLease for UnusedLease {
    type Error = Infallible;
    fn require_live(&self) -> Result<(), Self::Error> {
        panic!("oversized original must be refused before authority use")
    }
    fn deadline(&self) -> Instant {
        panic!("oversized original must be refused before authority use")
    }
    async fn withdrawn(&self) {
        panic!("oversized original must be refused before authority use")
    }
}

impl SessionAdmission for NoEffects {
    type Error = Infallible;
    type Lease = UnusedLease;
    async fn authorize_source(
        &self,
        _caller: &SubjectPrincipal,
        _key: &SessionKey,
    ) -> Result<Self::Lease, Self::Error> {
        panic!("private sealing guard must not authorize new work")
    }
    async fn authorize_create(&self, _material: CreateAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        panic!("private sealing guard must not authorize new work")
    }
    async fn authorize_dispatch(&self, _material: DispatchAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        panic!("private sealing guard must not authorize new work")
    }
    async fn authorize_child(&self, _material: ChildAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        panic!("private sealing guard must not authorize new work")
    }
}

impl SessionFactCodec for NoEffects {
    type Error = Infallible;
    async fn seal(
        &self,
        _key: &SessionKey,
        _event_id: EventId,
        _event_type: &str,
        _original: &[u8],
    ) -> Result<Vec<u8>, Self::Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("oversized original must not reach cryptographic custody")
    }
    async fn open(
        &self,
        _key: &SessionKey,
        _event_id: EventId,
        _event_type: &str,
        _envelope: &[u8],
    ) -> Result<Vec<u8>, Self::Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("oversized original must not decrypt private source material")
    }
}

#[tokio::test]
async fn final_sealing_boundary_refuses_oversized_original_before_any_owner_effect() {
    let effects = Arc::new(NoEffects::default());
    let runtime = SessionRuntime::new(Arc::clone(&effects), Arc::clone(&effects), Arc::clone(&effects));
    let key = SessionKey::new(trogon_nats::NatsToken::new("fixture").unwrap(), "parent").unwrap();
    let originals = [(wire::SessionStarted::FULL_NAME, vec![0; 131073])];
    let result = runtime
        .seal(
            &key,
            CommandId::new(uuid::Uuid::new_v4()),
            &originals,
            &[],
            Instant::now() + Duration::from_secs(15),
        )
        .await;
    assert!(matches!(result, Err(SessionError::Limit)));
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
}
