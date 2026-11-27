use crate::{AuthorityResourceIdError, AuthorityTaskCompletion};

/// The identity of a provider resource a worker holds for a lease.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct AuthorityResourceId {
    pub(crate) value: String,
    pub(crate) completion: Option<AuthorityTaskCompletion>,
}

impl AuthorityResourceId {
    /// The most bytes an identity may have.
    pub const MAX_LEN: usize = 4096;

    /// A resource identity that is nonempty, bounded and free of NUL.
    pub fn new(value: impl Into<String>) -> Result<Self, AuthorityResourceIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(AuthorityResourceIdError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(AuthorityResourceIdError::TooLong { max: Self::MAX_LEN });
        }
        if value.contains('\0') {
            return Err(AuthorityResourceIdError::NulByte);
        }
        Ok(Self {
            value,
            completion: None,
        })
    }

    /// The identity as text.
    pub fn as_str(&self) -> &str {
        &self.value
    }
}
