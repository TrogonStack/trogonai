use std::sync::Arc;
use std::time::SystemTime;

use nkeys::XKey;
use trogon_std::SecretString;
use trogonai_proto::secrets::exchange::v1alpha1::SealedAccess;

use super::constants::RELEASE_HOLD_BOUND;
use super::{InvalidReleaseResponseError, ReleaseError, ServiceXkey, UnsealError};

/// A credential value the secrets service released to this caller, opened.
///
/// The value may be held until [`ReleasedValue::hold_until`]: the earlier of
/// the service's `valid_until` and [`RELEASE_HOLD_BOUND`] after it arrived.
#[derive(Clone, Debug)]
pub struct ReleasedValue {
    value: SecretString,
    valid_until: SystemTime,
    bundle_generation: u64,
    hold_until: SystemTime,
}

impl ReleasedValue {
    /// Opens `access` as sealed to `requester` by the pinned `service` xkey,
    /// as it arrived at `now`.
    pub(crate) fn open(
        access: SealedAccess,
        requester: &XKey,
        service: &ServiceXkey,
        now: SystemTime,
    ) -> Result<Self, ReleaseError> {
        if access.xkey_sealed_value.is_empty() {
            return Err(InvalidReleaseResponseError::EmptySealedValue.into());
        }
        let valid_until = access
            .valid_until
            .into_option()
            .ok_or(InvalidReleaseResponseError::MissingValidUntil)?;
        if valid_until.seconds <= 0 {
            return Err(InvalidReleaseResponseError::ValidUntilNotPositive.into());
        }
        let valid_until = SystemTime::try_from(valid_until).map_err(InvalidReleaseResponseError::ValidUntil)?;
        if valid_until <= now {
            return Err(ReleaseError::Expired);
        }
        let opened = requester
            .open(&access.xkey_sealed_value, service.key())
            .map_err(|error| UnsealError::NotOpened(Arc::new(error)))?;
        let opened = String::from_utf8(opened).map_err(|_| UnsealError::NotUtf8)?;
        let value = SecretString::new(opened).map_err(|_| UnsealError::Empty)?;
        let hold_until = now
            .checked_add(RELEASE_HOLD_BOUND)
            .map_or(valid_until, |bound| bound.min(valid_until));
        Ok(Self {
            value,
            valid_until,
            bundle_generation: access.bundle_generation,
            hold_until,
        })
    }

    pub fn value(&self) -> &SecretString {
        &self.value
    }

    /// When the secrets service said the value stops being valid.
    pub const fn valid_until(&self) -> SystemTime {
        self.valid_until
    }

    /// The committed generation the value came from; zero for a kind that
    /// has no generation.
    pub const fn bundle_generation(&self) -> u64 {
        self.bundle_generation
    }

    /// The last moment the value may be held.
    pub const fn hold_until(&self) -> SystemTime {
        self.hold_until
    }

    /// Whether the value may still be used at `now`.
    pub fn is_held_at(&self, now: SystemTime) -> bool {
        now < self.hold_until
    }
}

#[cfg(test)]
mod tests;
