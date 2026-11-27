use bytes::Bytes;

use super::{ActorEntry, ActorIndexError, ActorIndexResult};

pub(super) fn decode_entry(key: &str, bytes: &[u8]) -> ActorIndexResult<ActorEntry> {
    let entry: ActorEntry = serde_json::from_slice(bytes).map_err(ActorIndexError::InvalidEntry)?;
    if entry.key() != key {
        return Err(ActorIndexError::EntryKeyMismatch);
    }
    Ok(entry)
}

pub(super) fn encode_entry(entry: &ActorEntry) -> ActorIndexResult<Bytes> {
    serde_json::to_vec(entry)
        .map(Bytes::from)
        .map_err(ActorIndexError::Encode)
}
