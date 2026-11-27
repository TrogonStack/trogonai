/// Why worker settings were refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SpaceConfigError {
    /// A duration that must be positive is zero.
    #[error("the {0} must not be zero")]
    ZeroDuration(&'static str),
}
