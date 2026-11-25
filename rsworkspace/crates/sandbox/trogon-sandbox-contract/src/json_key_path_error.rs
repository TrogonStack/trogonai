/// A list of keys is not a path [`JsonKeyPath`](crate::JsonKeyPath) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum JsonKeyPathError {
    #[error("a JSON key path must contain at least one key")]
    Empty,
}
