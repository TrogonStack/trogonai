use super::{CollaborationRef, EnrollmentIdentity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentFacts {
    pub(in crate::commands) identity: EnrollmentIdentity,
    pub(in crate::commands) collaboration: CollaborationRef,
}

impl EnrollmentFacts {
    pub fn identity(&self) -> &EnrollmentIdentity {
        &self.identity
    }

    pub fn collaboration(&self) -> &CollaborationRef {
        &self.collaboration
    }
}
