//! # trogon-transcript
//!
//! Persistent, append-only audit trail for every agent session in the
//! dynamic multi-agent system.
//!
//! ## Overview
//!
//! Every message, tool call, routing decision, and sub-agent spawn that occurs
//! inside an Entity Actor or the Router Agent is recorded as a [`TranscriptEntry`]
//! and published to the `TRANSCRIPTS` JetStream stream. Entries are stored under
//! a hierarchical subject:
//!
//! ```text
//! transcripts.{actor_type}.{actor_key}.{session_id}
//! ```
//!
//! This means:
//! - All sessions for a given entity can be queried with a wildcard filter.
//! - A single invocation can be replayed by its `session_id`.
//! - The full history of an entity (e.g. PR #456 from Monday through Friday) is
//!   queryable as a contiguous time-ordered series.
//!
//! ## Writing
//!
//! Use [`Session`] to write entries during a single actor invocation:
//!
//! ```rust,no_run
//! use trogon_transcript::{
//!     publisher::NatsTranscriptPublisher,
//!     session::Session,
//! };
//!
//! async fn example(js: async_nats::jetstream::Context) {
//!     let publisher = NatsTranscriptPublisher::new(js);
//!     let session = Session::new(publisher, "pr", "owner/repo/456");
//!
//!     session.append_user_message("Review this PR.", None).await.unwrap();
//!     session.append_assistant_message("LGTM", Some(42)).await.unwrap();
//! }
//! ```
//!
//! ## Reading
//!
//! Use [`TranscriptStore`] to read entries after the fact:
//!
//! ```rust,no_run
//! use trogon_transcript::store::TranscriptStore;
//!
//! async fn example(js: async_nats::jetstream::Context) {
//!     let store = TranscriptStore::new(js);
//!     store.provision().await.unwrap();
//!
//!     // All entries for PR #456 across all sessions
//!     let history = store.query("pr", "owner/repo/456").await.unwrap();
//!
//!     // Entries for a single session
//!     let session = store.replay("pr", "owner/repo/456", "some-session-id").await.unwrap();
//! }
//! ```
//!
//! ## Testing
//!
//! Enable the `test-support` feature to access [`publisher::mock::MockTranscriptPublisher`],
//! which collects entries in memory so you can assert on them without a NATS server.

pub mod entry;
pub mod error;
pub mod publisher;
pub mod reader;
pub mod session;
pub mod store;
pub mod subject;

pub use entry::{Role, TranscriptEntry, now_ms};
pub use error::TranscriptError;
pub use publisher::NatsTranscriptPublisher;
pub use publisher::TranscriptPublisher;
pub use reader::{NatsTranscriptReader, TranscriptRead};
pub use session::Session;
pub use store::TranscriptStore;
pub use subject::{entity_subject_filter, sanitize_key, transcript_subject};
