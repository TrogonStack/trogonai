use trogon_sandbox_contract::{DenyReason, EgressViolationReason};

pub(super) struct Refused {
    pub(super) reason: EgressViolationReason,
    pub(super) explanation: Option<DenyReason>,
}

impl From<EgressViolationReason> for Refused {
    fn from(reason: EgressViolationReason) -> Self {
        Self {
            reason,
            explanation: None,
        }
    }
}
