use bytes::Bytes;

pub(super) fn encode<T: serde::Serialize>(value: &T) -> Result<Bytes, serde_json::Error> {
    Ok(Bytes::from(serde_json::to_vec(value)?))
}
