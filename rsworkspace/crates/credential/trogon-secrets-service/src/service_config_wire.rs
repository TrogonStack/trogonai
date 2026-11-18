use std::path::Path;

use serde::Deserialize;
use trogon_std::ReadFile;

use super::audit_section_wire::AuditSectionWire;
use super::nats_section_wire::NatsSectionWire;
use super::openbao_section_wire::OpenBaoSectionWire;
use super::release_section_wire::ReleaseSectionWire;
use super::{ServiceConfigError, TomlSyntax};

/// The configuration file as written. Every section may be left out, and a
/// key no section defines is refused rather than ignored, so a misspelt
/// setting cannot fall back to its default unnoticed (ADR#0007).
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ServiceConfigWire {
    pub(crate) nats: NatsSectionWire,
    pub(crate) openbao: OpenBaoSectionWire,
    pub(crate) release: ReleaseSectionWire,
    pub(crate) audit: AuditSectionWire,
}

impl ServiceConfigWire {
    /// The configuration file at `path`, or no file at all when no path is
    /// given.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceConfigError`] when the file cannot be read or is not a
    /// configuration file, without quoting its lines.
    pub(crate) fn read(path: Option<&Path>, fs: &impl ReadFile) -> Result<Self, ServiceConfigError> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let document = fs
            .read_to_string(path)
            .map_err(|source| ServiceConfigError::ConfigUnreadable {
                path: path.to_owned(),
                source,
            })?;
        toml::from_str(&document).map_err(|error| ServiceConfigError::ConfigUnparsable {
            path: path.to_owned(),
            syntax: TomlSyntax::of(&error, &document),
        })
    }
}
