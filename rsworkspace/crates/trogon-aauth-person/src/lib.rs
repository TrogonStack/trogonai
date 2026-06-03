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
pub use store::{InMemoryStore, PersonStore};
