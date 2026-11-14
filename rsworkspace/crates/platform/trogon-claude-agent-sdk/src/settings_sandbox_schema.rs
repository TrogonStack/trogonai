//! The `SandboxNetworkConfigSchema`, `SandboxFilesystemConfigSchema` and
//! `SandboxCredentialsConfigSchema` Zod exports of the TypeScript SDK as Rust validators.
//!
//! Each validator runs the schema the pinned package builds, read from the extracted settings
//! description, so it accepts and rejects the same values and produces the same parsed output
//! (including key order) as `safeParse`.

use std::fmt;

use serde_json::Value;
use thiserror::Error;

use crate::settings_schema::schema;

/// One of the three sandbox configuration schemas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SandboxConfigSchema {
    /// `SandboxNetworkConfigSchema`.
    Network,
    /// `SandboxFilesystemConfigSchema`.
    Filesystem,
    /// `SandboxCredentialsConfigSchema`.
    Credentials,
}

/// The schema rejected the value.
#[derive(Debug, Error, Eq, PartialEq)]
#[error("{0} rejected the value")]
pub struct SandboxConfigRejected(pub SandboxConfigSchema);

impl SandboxConfigSchema {
    /// Every schema, in declaration order.
    pub const ALL: [Self; 3] = [Self::Network, Self::Filesystem, Self::Credentials];

    /// The name of the TypeScript export.
    #[must_use]
    pub const fn upstream_name(self) -> &'static str {
        match self {
            Self::Network => "SandboxNetworkConfigSchema",
            Self::Filesystem => "SandboxFilesystemConfigSchema",
            Self::Credentials => "SandboxCredentialsConfigSchema",
        }
    }

    const fn root(self) -> &'static str {
        match self {
            Self::Network => "sandboxNetwork",
            Self::Filesystem => "sandboxFilesystem",
            Self::Credentials => "sandboxCredentials",
        }
    }

    /// `safeParse(value).success`; `None` is `undefined`.
    #[must_use]
    pub fn accepts(self, value: Option<&Value>) -> bool {
        self.parse(value).is_ok()
    }

    /// `safeParse(value).data`: the parsed output, `Ok(None)` when it is `undefined`.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxConfigRejected`] when the schema rejects the value.
    pub fn parse(self, value: Option<&Value>) -> Result<Option<Value>, SandboxConfigRejected> {
        schema()
            .and_then(|schema| schema.root_output(self.root(), value))
            .ok_or(SandboxConfigRejected(self))
    }
}

impl fmt::Display for SandboxConfigSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.upstream_name())
    }
}

#[cfg(test)]
#[path = "settings_sandbox_schema_tests.rs"]
mod tests;
