use std::num::NonZeroU64;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialVersion(NonZeroU64);

impl CredentialVersion {
    pub fn new(value: u64) -> Result<Self, CredentialVersionError> {
        NonZeroU64::new(value).map(Self).ok_or(CredentialVersionError::Zero)
    }

    pub fn initial() -> Self {
        Self(NonZeroU64::MIN)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }

    pub fn next(self) -> Self {
        Self(self.0.checked_add(1).expect("credential version overflowed"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialVersionError {
    #[error("credential version must not be zero")]
    Zero,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_version_is_one() {
        assert_eq!(CredentialVersion::initial().get(), 1);
    }

    #[test]
    fn rejects_zero() {
        assert_eq!(CredentialVersion::new(0), Err(CredentialVersionError::Zero));
    }
}
