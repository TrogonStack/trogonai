//! AAuth Person Server: bootstraps agent identities and exchanges resource
//! challenge tokens for authorization tokens. Speaks HTTP and NATS; both
//! transports call into the same `PersonCore`.

#![allow(clippy::module_name_repetitions)]

pub mod core;
pub mod http;
pub mod nats;
pub mod policy;
pub mod store;

pub use core::{BootstrapRequest, BootstrapResponse, PersonCore, PersonError, TokenRequest, TokenResponse};
pub use policy::{AllowConfiguredScopes, ConsentDecision, ConsentPolicy};
pub use store::{
    AGENTS_BUCKET, AgentRecord, CONSENTS_BUCKET, ConsentRecord, InMemoryStore, JetStreamReplayStore, JetStreamStore,
    PersonStore, REPLAY_BUCKET, StoreError, agents_bucket_config, consents_bucket_config, replay_bucket_config,
};
