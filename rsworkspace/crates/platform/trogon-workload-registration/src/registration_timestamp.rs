use std::time::{Duration, SystemTime};

use buffa_types::google::protobuf::Timestamp;

use crate::RegistrationError;

#[derive(Debug)]
pub(crate) struct RegistrationTimestamp(SystemTime);

impl RegistrationTimestamp {
    pub(crate) fn decode(value: Option<&Timestamp>) -> Result<Self, RegistrationError> {
        let value = value.ok_or(RegistrationError::Lease)?;
        if value.seconds <= 0 || !(0..1_000_000_000).contains(&value.nanos) {
            return Err(RegistrationError::Lease);
        }
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::new(value.seconds as u64, value.nanos as u32))
            .map(Self)
            .ok_or(RegistrationError::Lease)
    }

    pub(crate) fn into_system_time(self) -> SystemTime {
        self.0
    }
}
