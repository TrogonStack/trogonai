use trogon_sandbox_contract::{DriveError, LeaseError};

/// A request refused by the authorization gate.
#[derive(Debug, PartialEq, Eq)]
pub struct Unauthorized {
    pub reason: String,
}

impl From<Unauthorized> for LeaseError {
    fn from(denied: Unauthorized) -> Self {
        LeaseError::Unauthorized { reason: denied.reason }
    }
}

impl From<Unauthorized> for DriveError {
    fn from(denied: Unauthorized) -> Self {
        DriveError::Unauthorized { reason: denied.reason }
    }
}
