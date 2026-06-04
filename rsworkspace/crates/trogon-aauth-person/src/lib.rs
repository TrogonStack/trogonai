//! AAuth Person Server: bootstraps agent identities and exchanges resource
//! challenge tokens for authorization tokens. Speaks HTTP and NATS; both
//! transports call into the same `PersonCore`.

#![allow(clippy::module_name_repetitions)]

pub mod claim_mapping;
pub mod core;
pub mod http;
pub mod nats;
pub mod policy;
pub mod store;
pub mod wif;
pub mod wif_admin;
pub mod wif_exchange;

pub use core::{BootstrapRequest, BootstrapResponse, PersonCore, PersonError, TokenRequest, TokenResponse};
pub use policy::{AllowConfiguredScopes, ConsentDecision, ConsentPolicy};
pub use store::{
    AGENTS_BUCKET, AgentRecord, CONSENTS_BUCKET, ConsentRecord, InMemoryStore, JetStreamReplayStore, JetStreamStore,
    PersonStore, REPLAY_BUCKET, StoreError, agents_bucket_config, consents_bucket_config, replay_bucket_config,
};
pub use wif::{
    ClaimMapping, InMemoryWifStore, JetStreamWifStore, KeySource, LifecycleState, MAPPINGS_BUCKET, MappingResolution,
    MatchPattern, PROVIDERS_BUCKET, ServiceAccountMappingRecord, WifStore, WorkloadIdentityProviderRecord,
    mappings_bucket_config, providers_bucket_config, resolve_mapping,
};
pub use wif_exchange::{
    GRANT_TYPE_TOKEN_EXCHANGE, ISSUED_TOKEN_TYPE_ACCESS_TOKEN, ResolvedWifExchange, SUBJECT_TOKEN_TYPE_ID_TOKEN,
    SUBJECT_TOKEN_TYPE_JWT, WifExchangeError, WifExchangeRequest, WifExchangeService,
};
