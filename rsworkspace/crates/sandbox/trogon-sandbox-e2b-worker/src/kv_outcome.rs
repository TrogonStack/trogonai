use async_nats::jetstream::kv::{CreateError, CreateErrorKind, UpdateError, UpdateErrorKind};

/// The revision a create produced, or `None` when another writer created the key first.
pub(crate) fn created(result: Result<u64, CreateError>) -> Result<Option<u64>, CreateError> {
    match result {
        Ok(revision) => Ok(Some(revision)),
        Err(error) if error.kind() == CreateErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(error),
    }
}

/// The revision an update produced, or `None` when the key moved past the revision the update named.
pub(crate) fn updated(result: Result<u64, UpdateError>) -> Result<Option<u64>, UpdateError> {
    match result {
        Ok(revision) => Ok(Some(revision)),
        Err(error) if error.kind() == UpdateErrorKind::WrongLastRevision => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
