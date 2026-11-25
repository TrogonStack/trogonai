use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ProviderKind;

/// Everything that makes one execution environment different from another: the provider, host architecture,
/// pinned runtime components, boot inputs, snapshot format, isolation profile, and CPU template.
///
/// Field order is part of the wire shape, and `components` is a sorted map. A change to a field or to its meaning
/// bumps [`EnvironmentDefinition::SCHEMA_VERSION`]. The definition describes an environment; it does not compute
/// the [`EnvDigest`](crate::EnvDigest) of one.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentDefinition {
    pub schema_version: u32,
    pub provider: ProviderKind,
    /// Host CPU architecture, such as `x86_64` or `aarch64`.
    pub architecture: String,
    /// Named runtime pieces and what pins each: a version string or a `sha256:` digest.
    pub components: BTreeMap<String, String>,
    /// Kernel command line the guest boots with.
    pub boot_args: String,
    /// Version of the snapshot file format the worker writes and reads.
    pub snapshot_format: u32,
    /// How the VMM is confined, for example `jailer` or `direct`.
    pub isolation_profile: String,
    /// CPU template applied to the guest, if any.
    pub cpu_template: Option<String>,
}

impl EnvironmentDefinition {
    /// Version of the definition's encoding. Bump it when a field is added or its meaning changes.
    pub const SCHEMA_VERSION: u32 = 1;

    /// Starts a definition at the current schema version with no components, boot arguments, or CPU template.
    pub fn new(provider: ProviderKind, architecture: impl Into<String>) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            provider,
            architecture: architecture.into(),
            components: BTreeMap::new(),
            boot_args: String::new(),
            snapshot_format: 0,
            isolation_profile: String::new(),
            cpu_template: None,
        }
    }

    /// Pins one named component, replacing an earlier pin of the same name.
    pub fn with_component(mut self, name: impl Into<String>, pin: impl Into<String>) -> Self {
        self.components.insert(name.into(), pin.into());
        self
    }
}
