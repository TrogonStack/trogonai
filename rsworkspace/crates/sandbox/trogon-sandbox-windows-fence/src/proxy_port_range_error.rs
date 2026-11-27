/// A string or a pair of ports is not a range
/// [`ProxyPortRange`](crate::ProxyPortRange) accepts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProxyPortRangeError {
    #[error("low port must be >= 1")]
    ZeroLow,
    #[error("low ({low}) > high ({high})")]
    Inverted { low: u16, high: u16 },
    #[error("range too wide ({ports} ports); max width {max}")]
    TooWide { ports: u32, max: u32 },
    #[error("expected LOW-HIGH (e.g. 60080-60089)")]
    MissingSeparator,
    #[error("invalid low port '{0}'")]
    InvalidLow(String),
    #[error("invalid high port '{0}'")]
    InvalidHigh(String),
}
