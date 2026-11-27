use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{AwsCredentialPairBinding, InjectHosts, InjectScope, ResponseHeaderStrip, SecretName};

use crate::StripRule;
use crate::registry_entry::RegistryEntry;
use crate::{HandoffError, RealValue, Sentinel, SentinelRegistry};

fn is_default_scope(scope: &&InjectScope) -> bool {
    **scope == InjectScope::default()
}

#[derive(Serialize)]
struct EntryOut<'a> {
    name: &'a SecretName,
    sentinel: &'a str,
    real: &'a str,
    inject_hosts: &'a InjectHosts,
    #[serde(skip_serializing_if = "is_default_scope")]
    scope: &'a InjectScope,
}

#[derive(Deserialize)]
struct EntryIn {
    name: SecretName,
    sentinel: String,
    real: String,
    inject_hosts: InjectHosts,
    #[serde(default)]
    scope: InjectScope,
}

#[derive(Serialize)]
struct StripOut<'a> {
    hosts: &'a InjectHosts,
    headers: &'a ResponseHeaderStrip,
}

#[derive(Deserialize)]
struct StripIn {
    hosts: InjectHosts,
    headers: ResponseHeaderStrip,
}

#[derive(Serialize)]
struct HandoffOut<'a> {
    entries: Vec<EntryOut<'a>>,
    aws_pairs: &'a [AwsCredentialPairBinding],
    strip_rules: Vec<StripOut<'a>>,
}

#[derive(Deserialize)]
struct HandoffIn {
    entries: Vec<EntryIn>,
    #[serde(default)]
    aws_pairs: Vec<AwsCredentialPairBinding>,
    #[serde(default)]
    strip_rules: Vec<StripIn>,
}

/// Carries a [`SentinelRegistry`] from the process that minted it to the lease proxy child, so both sides agree on
/// every sentinel.
///
/// The encoded form holds real values. It lives in a `SecretString`, has a redacted `Debug`, and is meant to travel
/// only over an inherited pipe, never argv, never the environment and never a file.
pub struct RegistryHandoff(SecretString);

impl RegistryHandoff {
    /// Encodes `registry`, real values included.
    pub fn from_registry(registry: &SentinelRegistry) -> Result<Self, HandoffError> {
        let entries: Vec<EntryOut<'_>> = registry
            .entries
            .iter()
            .map(|entry| EntryOut {
                name: &entry.name,
                sentinel: entry.sentinel.as_str(),
                real: entry.real.expose(),
                inject_hosts: &entry.inject_hosts,
                scope: &entry.scope,
            })
            .collect();
        let json = serde_json::to_string(&HandoffOut {
            entries,
            aws_pairs: &registry.aws_pairs,
            strip_rules: registry
                .strip_rules
                .iter()
                .map(|rule| StripOut {
                    hosts: &rule.hosts,
                    headers: &rule.headers,
                })
                .collect(),
        })
        .map_err(|_| HandoffError::Unencodable)?;
        Ok(Self(SecretString::from(json)))
    }

    /// Wraps the encoded form received over the pipe.
    pub fn from_wire(wire: String) -> Self {
        Self(SecretString::from(wire))
    }

    /// The encoded form to write to the pipe.
    pub fn expose_wire(&self) -> &str {
        self.0.expose_secret()
    }

    /// Rebuilds the registry. Anything that does not decode to a consistent registry is [`HandoffError::Malformed`].
    pub fn into_registry(self) -> Result<SentinelRegistry, HandoffError> {
        let handoff: HandoffIn = serde_json::from_str(self.0.expose_secret()).map_err(|_| HandoffError::Malformed)?;
        let mut registry = SentinelRegistry::new();
        for entry in handoff.entries {
            let sentinel = Sentinel::restore(entry.sentinel).ok_or(HandoffError::Malformed)?;
            if registry.entry(&entry.name).is_some() {
                return Err(HandoffError::Malformed);
            }
            registry.entries.push(RegistryEntry {
                name: entry.name,
                sentinel,
                real: RealValue::new(entry.real),
                inject_hosts: entry.inject_hosts,
                scope: entry.scope,
            });
        }
        for pair in &handoff.aws_pairs {
            registry.link_aws_pair(pair).map_err(|_| HandoffError::Malformed)?;
        }
        for rule in handoff.strip_rules {
            registry.strip_rules.push(StripRule {
                hosts: rule.hosts,
                headers: rule.headers,
            });
        }
        Ok(registry)
    }
}

impl fmt::Debug for RegistryHandoff {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RegistryHandoff(***)")
    }
}
