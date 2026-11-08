//! The first [`PolicySnapshot`]: an operator-authored TOML file (ADR#0085
//! Decision 6).
//!
//! ```toml
//! [agents.agent-1]
//! allow = ["api.example.com:443", "10.0.0.5:8080", "[2001:db8::1]:443"]
//! ```
//!
//! Each entry is an exact `host:port`, parsed by the same [`Destination`] type
//! the CONNECT target uses: no wildcards, paths or spaces, port 1 to 65535.
//! A file that is missing, unreadable, unparsable or that permits nothing is a
//! typed error, because a proxy with no policy must not start (ADR#0084).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::agent_id::{AgentId, AgentIdError};
use crate::destination::{Destination, DestinationError};
use crate::policy_snapshot::{PolicySnapshot, SnapshotVerdict};
use crate::policy_source::PolicySource;

#[derive(Debug)]
pub struct StaticSnapshot {
    source: PolicySource,
    permits: HashMap<AgentId, HashSet<Destination>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileWire {
    #[serde(default)]
    agents: BTreeMap<String, AgentWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentWire {
    allow: Vec<String>,
}

impl StaticSnapshot {
    /// Reads and validates the file at `path`.
    pub fn load(path: &Path) -> Result<Self, SnapshotError> {
        let text = std::fs::read_to_string(path).map_err(|source| SnapshotError::Unreadable {
            path: path.to_owned(),
            source,
        })?;
        Self::from_toml(PolicySource::StaticFile { path: path.to_owned() }, &text)
    }

    pub fn from_toml(source: PolicySource, text: &str) -> Result<Self, SnapshotError> {
        let wire: FileWire = toml::from_str(text).map_err(|error| SnapshotError::Unparsable {
            source: Box::new(error),
        })?;
        if wire.agents.is_empty() {
            return Err(SnapshotError::NoAgents);
        }
        let mut permits = HashMap::with_capacity(wire.agents.len());
        for (raw_agent, entry) in wire.agents {
            let agent = AgentId::new(&raw_agent).map_err(|source| SnapshotError::InvalidAgent {
                agent: raw_agent.clone(),
                source,
            })?;
            if entry.allow.is_empty() {
                return Err(SnapshotError::EmptyPermitList { agent });
            }
            let mut destinations = HashSet::with_capacity(entry.allow.len());
            for raw in entry.allow {
                let destination = raw
                    .parse::<Destination>()
                    .map_err(|source| SnapshotError::InvalidDestination {
                        agent: agent.clone(),
                        entry: raw.clone(),
                        source,
                    })?;
                destinations.insert(destination);
            }
            permits.insert(agent, destinations);
        }
        Ok(Self { source, permits })
    }

    pub fn agent_count(&self) -> usize {
        self.permits.len()
    }

    pub fn permit_count(&self) -> usize {
        self.permits.values().map(HashSet::len).sum()
    }
}

impl PolicySnapshot for StaticSnapshot {
    fn source(&self) -> &PolicySource {
        &self.source
    }

    fn lookup(&self, agent: &AgentId, destination: &Destination, _now: SystemTime) -> SnapshotVerdict {
        match self.permits.get(agent) {
            Some(destinations) if destinations.contains(destination) => SnapshotVerdict::Permitted,
            _ => SnapshotVerdict::NotPermitted,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("policy file {path} could not be read")]
    Unreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("policy file is not valid")]
    Unparsable {
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("policy file permits no agent, so every connection would be denied")]
    NoAgents,
    #[error("policy file names an invalid agent id {agent:?}")]
    InvalidAgent {
        agent: String,
        #[source]
        source: AgentIdError,
    },
    #[error("agent {agent} has an empty allow list")]
    EmptyPermitList { agent: AgentId },
    #[error("agent {agent} has an invalid destination {entry:?}")]
    InvalidDestination {
        agent: AgentId,
        entry: String,
        #[source]
        source: DestinationError,
    },
}

#[cfg(test)]
mod tests;
