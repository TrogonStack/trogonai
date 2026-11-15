use serde::Deserialize;

/// The `[audit]` section as written: who the service's audit writer is.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuditSectionWire {
    pub(crate) tenant: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) instance: Option<String>,
}
