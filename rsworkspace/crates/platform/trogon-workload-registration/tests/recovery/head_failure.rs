use trogon_workload_registration::{PublicationReceipt, RegistrationError};

pub enum HeadFailure {
    Scope,
    Lease,
    Bounds,
    Noncanonical,
    Revision,
    ApplicationIdentity,
}

impl HeadFailure {
    pub fn assert_refused(self, result: Result<PublicationReceipt, RegistrationError>) {
        assert!(
            matches!(
                (self, result),
                (Self::Scope, Err(RegistrationError::Scope))
                    | (Self::Lease, Err(RegistrationError::Lease))
                    | (Self::Bounds, Err(RegistrationError::Bounds))
                    | (Self::Noncanonical, Err(RegistrationError::NoncanonicalPayload))
                    | (Self::Revision, Err(RegistrationError::Revision))
                    | (Self::ApplicationIdentity, Err(RegistrationError::ApplicationIdentity))
            ),
            "untrusted durable head must refuse the withdrawal before any live publication"
        );
    }
}
