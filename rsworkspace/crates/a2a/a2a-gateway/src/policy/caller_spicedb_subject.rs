use std::fmt;

use a2a_auth_callout::SpiceDbSubject;
use trogon_identity_types::{PrincipalKind, SubjectPrincipal};

/// The SpiceDB subject a policy tier checks for the caller the ingress
/// subject names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallerSpiceDbSubject(SubjectPrincipal);

impl CallerSpiceDbSubject {
    pub fn object_type(&self) -> &'static str {
        match self.0.kind() {
            PrincipalKind::Person => "user",
            PrincipalKind::Agent => "agent",
            PrincipalKind::Service => "service",
        }
    }

    pub fn object_id(&self) -> &str {
        self.0.id().as_str()
    }

    pub fn to_spicedb_subject(&self) -> SpiceDbSubject {
        SpiceDbSubject::new(self.to_string())
    }
}

impl From<&SubjectPrincipal> for CallerSpiceDbSubject {
    fn from(caller: &SubjectPrincipal) -> Self {
        Self(caller.clone())
    }
}

impl fmt::Display for CallerSpiceDbSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.object_type(), self.object_id())
    }
}

#[cfg(test)]
mod tests;
