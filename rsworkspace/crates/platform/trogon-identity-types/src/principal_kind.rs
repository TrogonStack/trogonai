use std::fmt;
use std::str::FromStr;

/// What kind of actor a principal represents.
///
/// Kept closed rather than open: an authorizer that must branch on the actor
/// type can only do so exhaustively if the set is known, and a fourth kind is
/// a decision ADR#0026 should be revisited for rather than a value a caller
/// invents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrincipalKind {
    /// An autonomous agent acting on its own behalf.
    Agent,
    /// A human, whether acting directly or through a client.
    Person,
    /// A non-agent workload, such as a scheduler or a background processor.
    Service,
}

impl PrincipalKind {
    /// Returns the stable wire name of this kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Person => "person",
            Self::Service => "service",
        }
    }
}

impl fmt::Display for PrincipalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl FromStr for PrincipalKind {
    type Err = PrincipalKindError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "agent" => Ok(Self::Agent),
            "person" => Ok(Self::Person),
            "service" => Ok(Self::Service),
            "" => Err(PrincipalKindError::Empty),
            _ => Err(PrincipalKindError::Unknown),
        }
    }
}

impl TryFrom<&str> for PrincipalKind {
    type Error = PrincipalKindError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl serde::Serialize for PrincipalKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for PrincipalKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        wire.parse().map_err(serde::de::Error::custom)
    }
}

/// Error returned when a wire name does not name a [`PrincipalKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrincipalKindError {
    /// The wire name was empty.
    #[error("principal kind cannot be empty")]
    Empty,
    /// The wire name is not `agent`, `person` or `service`.
    #[error("principal kind must be agent, person or service")]
    Unknown,
}

#[cfg(test)]
mod tests;
