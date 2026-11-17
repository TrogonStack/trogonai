use serde::Deserialize;

use super::audit_section_wire::AuditSectionWire;
use super::nats_section_wire::NatsSectionWire;
use super::openbao_section_wire::OpenBaoSectionWire;
use super::release_section_wire::ReleaseSectionWire;

/// The configuration file as written. Every section may be left out, and a
/// key no section defines is refused rather than ignored, so a misspelt
/// setting cannot fall back to its default unnoticed (ADR#0007).
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ServiceConfigWire {
    pub(crate) nats: NatsSectionWire,
    pub(crate) openbao: OpenBaoSectionWire,
    pub(crate) release: ReleaseSectionWire,
    pub(crate) audit: AuditSectionWire,
}
