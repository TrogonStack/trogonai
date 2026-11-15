use std::time::SystemTime;

use trogon_credential_domain::CredentialVersion;

use super::{ReleaseRequest, RequesterXkey};

/// The audit record of a release, written before the value is sealed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseRecord {
    request: ReleaseRequest,
    version: CredentialVersion,
    recipient: RequesterXkey,
    valid_until: SystemTime,
}

impl ReleaseRecord {
    pub fn new(
        request: ReleaseRequest,
        version: CredentialVersion,
        recipient: RequesterXkey,
        valid_until: SystemTime,
    ) -> Self {
        Self {
            request,
            version,
            recipient,
            valid_until,
        }
    }

    pub fn request(&self) -> &ReleaseRequest {
        &self.request
    }

    /// The version whose value is released.
    pub fn version(&self) -> CredentialVersion {
        self.version
    }

    /// The xkey the value is sealed to.
    pub fn recipient(&self) -> &RequesterXkey {
        &self.recipient
    }

    pub fn valid_until(&self) -> SystemTime {
        self.valid_until
    }
}
