use serde::{Deserialize, Serialize};

use crate::{InjectScopeVersionError, Port};

/// Which meaning an inject host without a port has.
///
/// `V1` is the default and the meaning bindings had before versions existed: a pattern without a port matches every
/// port. `V2` makes a pattern without a port mean port 443. It serializes as the JSON number `1` or `2`, and a binding
/// omits it from the wire when it is the default.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum InjectScopeVersion {
    #[default]
    V1,
    V2,
}

impl InjectScopeVersion {
    /// The port a host pattern without one stands for, or `None` when it spans every port.
    pub(crate) fn omitted_port(self) -> Option<Port> {
        match self {
            Self::V1 => None,
            Self::V2 => Some(Port::HTTPS),
        }
    }

    /// Reports whether this is the default version.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl TryFrom<u8> for InjectScopeVersion {
    type Error = InjectScopeVersionError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::V1),
            2 => Ok(Self::V2),
            other => Err(InjectScopeVersionError::Unknown(other)),
        }
    }
}

impl From<InjectScopeVersion> for u8 {
    fn from(version: InjectScopeVersion) -> Self {
        match version {
            InjectScopeVersion::V1 => 1,
            InjectScopeVersion::V2 => 2,
        }
    }
}
