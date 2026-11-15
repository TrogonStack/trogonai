#![allow(clippy::unwrap_used)]

use trogon_decider::Decider;
use trogon_decider_runtime::{
    PreconditionConflictError, StreamPosition, StreamWritePrecondition, resolve_write_precondition,
};
use trogon_peer::Enroll;

#[test]
fn enrollment_cannot_adopt_an_existing_stream_after_incomplete_replay() {
    let observed = StreamPosition::try_new(7).unwrap();
    assert_eq!(
        resolve_write_precondition(Enroll::WRITE_PRECONDITION, None, Some(observed)),
        Ok(StreamWritePrecondition::NoStream)
    );
}

#[test]
fn caller_revision_cannot_authorize_enrollment_in_an_existing_stream() {
    let revision = StreamPosition::try_new(7).unwrap();
    assert_eq!(
        resolve_write_precondition(Enroll::WRITE_PRECONDITION, Some(revision), Some(revision)),
        Err(PreconditionConflictError::CreateWithRevision)
    );
}
