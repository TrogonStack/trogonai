//! Event codec traits for domain event serialization.
//!
//! These traits define the bytes boundary between domain events and storage
//! adapters. They live in this crate (not `trogon-decider-runtime`) so guest
//! WASM modules can depend on them without pulling tokio, uuid, or chrono.

mod codec;

pub use codec::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventPayloadError, EventType};
