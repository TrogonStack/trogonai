use std::{error::Error, future::Future};

use trogon_decider_runtime::EventId;

use crate::SessionKey;

/// The application supplies canonical sealing with authenticated key, event id and concrete type.
/// No plaintext production adapter is supplied by the lifecycle owner.
pub trait SessionFactCodec: Send + Sync {
    type Error: Error + Send + Sync + 'static;

    fn seal(
        &self,
        key: &SessionKey,
        event_id: EventId,
        event_type: &str,
        original_plaintext: &[u8],
    ) -> impl Future<Output = Result<Vec<u8>, Self::Error>> + Send;

    fn open(
        &self,
        key: &SessionKey,
        event_id: EventId,
        event_type: &str,
        envelope: &[u8],
    ) -> impl Future<Output = Result<Vec<u8>, Self::Error>> + Send;
}
