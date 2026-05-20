//! Storage-neutral runtime contracts for Trogon deciders.
//!
//! This crate sits between pure decision logic from [`trogon_decider`] and the
//! storage adapters that persist or replay events. It owns the contracts whose
//! semantics must stay stable across backends: event envelopes, metadata
//! headers, stream reads, stream appends, and stream positions.
//!
//! The crate deliberately avoids choosing an execution loop, storage backend,
//! or checkpoint strategy. Applications compose those policies around these
//! primitives so adapters can remain thin translations to their native SDKs.
//!
//! # Position Semantics
//!
//! [`StreamPosition`] is a comparable stream high-watermark. It is not a
//! gapless revision, event count, or "next expected version". Callers may store
//! and compare it for concurrency and freshness checks, but should not do
//! arithmetic on it except through helpers such as [`ReadFrom::after`].
//!
//! # Example
//!
//! ```rust
//! use trogon_decider_runtime::{
//!     Event, EventId, Headers, StreamPosition, StreamWritePrecondition,
//! };
//! use uuid::Uuid;
//!
//! let event = Event {
//!     id: EventId::new(Uuid::now_v7()),
//!     r#type: "ExampleCreated".to_string(),
//!     content: br#"{"id":"example"}"#.to_vec(),
//!     headers: Headers::empty(),
//! };
//!
//! let observed = StreamPosition::try_new(1)?;
//! let precondition = StreamWritePrecondition::At(observed);
//! # let _ = (event, precondition);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

/// Event envelopes and codec traits used by stream storage adapters.
pub mod event;
/// Metadata header value objects carried alongside event payloads.
pub mod headers;
/// Stream read/write contracts shared by event store backends.
pub mod stream;

pub use event::{Event, EventData, EventDecode, EventEncode, EventId, EventIdentity, EventType, StreamEvent};
pub use headers::{FromEntriesError, HeaderName, HeaderNameError, HeaderValue, HeaderValueError, Headers};
pub use stream::{
    AppendStreamRequest, AppendStreamResponse, InvalidStreamPosition, ReadAfterOverflow, ReadFrom, ReadStreamRequest,
    ReadStreamResponse, StreamAppend, StreamPosition, StreamRead, StreamWritePrecondition,
};
#[cfg(feature = "test-support")]
pub use trogon_decider::testing;
#[cfg(feature = "test-support")]
pub use trogon_decider::testing::{History, TestCase, ThenError, ThenEvents, ThenExpectation};
pub use trogon_decider::{Act, ActBuilder, Decider, Decision, Events, WritePrecondition};
