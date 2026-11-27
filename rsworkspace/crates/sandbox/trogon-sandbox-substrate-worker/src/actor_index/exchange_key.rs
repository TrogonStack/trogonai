use super::{ActorEntry, ActorIndexError, ActorIndexResult};

/// The key a compare-and-exchange acts on, once both sides are known to name the same entry.
pub(super) fn exchange_key(
    expected: Option<&ActorEntry>,
    replacement: Option<&ActorEntry>,
) -> ActorIndexResult<String> {
    let key = expected
        .or(replacement)
        .ok_or(ActorIndexError::ExchangeWithoutIdentity)?
        .key();
    if !(expected.is_none_or(|entry| entry.key() == key) && replacement.is_none_or(|entry| entry.key() == key)) {
        return Err(ActorIndexError::ExchangeIdentitiesDiffer);
    }
    Ok(key)
}
