/// A number is not a [`TunnelCap`](crate::TunnelCap).
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum TunnelCapError {
    #[error("the tunnel cap must be between 1 and {max}")]
    OutOfRange { max: u32 },
}
