use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ReadyPortsError;

/// The sandbox-side ports a readiness wait checks.
///
/// There are at most [`ReadyPorts::MAX_LEN`], none is 0, and none repeats. An empty list checks the lease state only.
/// Construction and deserialization both enforce the rules. It serializes as a JSON array of numbers.
#[derive(Debug, Clone, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<u16>", into = "Vec<u16>")]
pub struct ReadyPorts(Vec<u16>);

impl ReadyPorts {
    /// Most ports one wait may name.
    pub const MAX_LEN: usize = 16;

    pub fn new(ports: Vec<u16>) -> Result<Self, ReadyPortsError> {
        if ports.len() > Self::MAX_LEN {
            return Err(ReadyPortsError::TooMany { max: Self::MAX_LEN });
        }
        if ports.contains(&0) {
            return Err(ReadyPortsError::ZeroPort);
        }
        let mut seen = BTreeSet::new();
        if !ports.iter().all(|port| seen.insert(*port)) {
            return Err(ReadyPortsError::Duplicate);
        }
        Ok(Self(ports))
    }

    pub fn as_slice(&self) -> &[u16] {
        &self.0
    }
}

impl TryFrom<Vec<u16>> for ReadyPorts {
    type Error = ReadyPortsError;

    fn try_from(ports: Vec<u16>) -> Result<Self, Self::Error> {
        Self::new(ports)
    }
}

impl From<ReadyPorts> for Vec<u16> {
    fn from(ports: ReadyPorts) -> Self {
        ports.0
    }
}
