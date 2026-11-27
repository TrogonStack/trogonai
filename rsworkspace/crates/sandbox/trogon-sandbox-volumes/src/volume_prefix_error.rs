/// A string is not a valid object-store prefix.
#[derive(Debug, thiserror::Error)]
#[error("invalid volume prefix: {0}")]
pub struct VolumePrefixError(#[from] pub object_store::path::Error);
