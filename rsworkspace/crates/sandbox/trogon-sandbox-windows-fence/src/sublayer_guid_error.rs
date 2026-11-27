/// A string is not a sublayer GUID name [`SublayerGuid`](crate::SublayerGuid) accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SublayerGuidError {
    #[error("sublayer GUID must not be empty")]
    Empty,
}
