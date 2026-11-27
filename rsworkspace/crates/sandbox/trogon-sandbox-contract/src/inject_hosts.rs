use serde::{Deserialize, Serialize};

use crate::{DomainPattern, InjectHostsError};

/// The hosts a binding may inject its credential at.
///
/// It holds at least one pattern. It serializes as a JSON array of domain patterns, and construction and
/// deserialization both reject an empty array, so a binding that injects nowhere cannot be expressed.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<DomainPattern>", into = "Vec<DomainPattern>")]
pub struct InjectHosts(Vec<DomainPattern>);

impl InjectHosts {
    pub fn new(hosts: Vec<DomainPattern>) -> Result<Self, InjectHostsError> {
        if hosts.is_empty() {
            return Err(InjectHostsError::Empty);
        }
        Ok(Self(hosts))
    }

    pub fn hosts(&self) -> &[DomainPattern] {
        &self.0
    }
}

impl TryFrom<Vec<DomainPattern>> for InjectHosts {
    type Error = InjectHostsError;

    fn try_from(hosts: Vec<DomainPattern>) -> Result<Self, Self::Error> {
        Self::new(hosts)
    }
}

impl From<InjectHosts> for Vec<DomainPattern> {
    fn from(hosts: InjectHosts) -> Self {
        hosts.0
    }
}
