/// A resource type name does not satisfy the rules [`TenantResourceType`](crate::TenantResourceType) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum TenantResourceTypeError {
    #[error("tenant resource type must not be empty")]
    Empty,
    #[error("tenant resource type must be at most {max} bytes")]
    TooLong { max: usize },
    #[error("tenant resource type must start with a lowercase ASCII letter")]
    InvalidStart,
    #[error("tenant resource type must contain only lowercase ASCII letters, digits, or underscores")]
    InvalidCharacter,
}
