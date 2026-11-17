#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! The read side of the credential aggregate.
//!
//! Each credential's events live on one subject of [`CREDENTIAL_STREAM`],
//! addressed by [`CredentialEventSubjectResolver`]. [`evolve_stream`] folds
//! them, from [`initial_state`], into the state snapshot the stream records,
//! and [`releasable_version`] reads off that snapshot the one version a
//! credential may release, if any. [`invalidation_reason`] reads off one event
//! whether it ends a release that rule handed out, and why, as an
//! [`InvalidationReason`], so holders are told to drop a value exactly when the
//! fold stops releasing it.
//!
//! The [`proto`] module is the wire codec for those events and states. Every
//! consumer that folds a credential stream depends on this crate, so the
//! gateway and the
//! [ADR#0074](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0074-credential-platform-extraction-boundary.md)
//! secrets service apply the same fold and the same releasability rule and
//! never disagree about whether a credential is releasable.

mod constants;
mod credential_event_subject_resolver;
mod credential_event_subject_resolver_error;
mod credential_evolve_error;
mod credential_field_invalid_error;
mod credential_field_path;
mod credential_proto_decode_error;
mod invalidation_reason;
pub mod proto;
mod releasable_version;
mod state;
mod unspecified_enum_kind;

pub use constants::{CREDENTIAL_EVENT_SUBJECT_PREFIX, CREDENTIAL_STREAM};
pub use credential_event_subject_resolver::{
    CredentialEventSubjectResolver, credential_event_subject, credential_project_token,
};
pub use credential_event_subject_resolver_error::CredentialEventSubjectResolverError;
pub use credential_evolve_error::CredentialEvolveError;
pub use credential_field_invalid_error::CredentialFieldInvalidError;
pub use credential_field_path::CredentialFieldPath;
pub use credential_proto_decode_error::CredentialProtoDecodeError;
pub use invalidation_reason::{InvalidationReason, invalidation_reason};
pub use releasable_version::releasable_version;
pub use state::{evolve, evolve_stream, initial_state};
pub use unspecified_enum_kind::UnspecifiedEnumKind;
