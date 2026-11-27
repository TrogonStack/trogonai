use async_nats::jetstream::kv::{CreateError, CreateErrorKind, UpdateError, UpdateErrorKind};

use super::{created, updated};

#[test]
fn a_create_yields_its_revision() {
    assert!(matches!(created(Ok(4)), Ok(Some(4))));
}

#[test]
fn a_lost_create_race_asks_for_another_round() {
    assert!(matches!(
        created(Err(CreateError::new(CreateErrorKind::AlreadyExists))),
        Ok(None)
    ));
}

#[test]
fn any_other_create_failure_is_kept() {
    assert!(created(Err(CreateError::new(CreateErrorKind::InvalidKey))).is_err());
}

#[test]
fn an_update_yields_its_revision() {
    assert!(matches!(updated(Ok(9)), Ok(Some(9))));
}

#[test]
fn a_stale_revision_asks_for_another_round() {
    assert!(matches!(
        updated(Err(UpdateError::new(UpdateErrorKind::WrongLastRevision))),
        Ok(None)
    ));
}

#[test]
fn any_other_update_failure_is_kept() {
    assert!(updated(Err(UpdateError::new(UpdateErrorKind::InvalidKey))).is_err());
}
