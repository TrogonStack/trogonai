/// A number is not an inject scope version [`InjectScopeVersion`](crate::InjectScopeVersion) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum InjectScopeVersionError {
    #[error("unsupported scope_version {0}, expected 1 or 2")]
    Unknown(u8),
}
