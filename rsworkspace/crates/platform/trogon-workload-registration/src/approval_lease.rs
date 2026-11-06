use std::time::{Instant, SystemTime};

use trogon_workload_identity::constants::MAX_REGISTRATION_LIFETIME;

use crate::RegistrationError;

#[derive(Debug, Clone)]
pub struct ApprovalLease {
    issued_at: SystemTime,
    expires_at: SystemTime,
    deadline: Instant,
}

impl ApprovalLease {
    pub(crate) fn new(
        issued_at: SystemTime,
        expires_at: SystemTime,
        started: Instant,
        now: SystemTime,
    ) -> Result<Self, RegistrationError> {
        let lifetime = expires_at
            .duration_since(issued_at)
            .map_err(|_| RegistrationError::Lease)?;
        let remaining = expires_at.duration_since(now).map_err(|_| RegistrationError::Lease)?;
        if issued_at > now || lifetime.is_zero() || lifetime > MAX_REGISTRATION_LIFETIME || remaining.is_zero() {
            return Err(RegistrationError::Lease);
        }
        let deadline = started.checked_add(remaining).ok_or(RegistrationError::Lease)?;
        let lease = Self {
            issued_at,
            expires_at,
            deadline,
        };
        lease.check_current()?;
        Ok(lease)
    }

    pub fn issued_at(&self) -> SystemTime {
        self.issued_at
    }
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }

    pub fn check_current(&self) -> Result<(), RegistrationError> {
        let instant = Instant::now();
        let now = SystemTime::now();
        if instant >= self.deadline || now < self.issued_at || now >= self.expires_at {
            return Err(RegistrationError::Lease);
        }
        Ok(())
    }
}
