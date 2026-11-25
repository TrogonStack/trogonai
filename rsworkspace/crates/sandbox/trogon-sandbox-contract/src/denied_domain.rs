use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{DomainPattern, DomainPatternError, PatternTarget, Port};

/// An entry of a deny list.
///
/// Unlike a [`DomainPattern`] it may be a bare `*`, optionally with a port such as `*:22`, to deny every
/// destination. It serializes as its canonical string, and construction, parsing, and deserialization all enforce
/// the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeniedDomain(DomainPattern);

impl DeniedDomain {
    pub fn new(input: &str) -> Result<Self, DomainPatternError> {
        DomainPattern::parse_with(input, true).map(Self)
    }

    /// What the entry names, which is every destination for a bare `*`.
    pub fn target(&self) -> &PatternTarget {
        self.0.target()
    }

    /// The port the entry is limited to, or `None` for every port.
    pub fn port(&self) -> Option<Port> {
        self.0.port()
    }
}

impl TryFrom<String> for DeniedDomain {
    type Error = DomainPatternError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl FromStr for DeniedDomain {
    type Err = DomainPatternError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<DeniedDomain> for String {
    fn from(domain: DeniedDomain) -> Self {
        domain.to_string()
    }
}

impl fmt::Display for DeniedDomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
