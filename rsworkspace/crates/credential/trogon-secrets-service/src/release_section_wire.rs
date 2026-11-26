use serde::Deserialize;

/// The `[release]` section as written.
///
/// The sealing key seed is read as any TOML value, for the reason the NATS
/// secrets are.
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseSectionWire {
    pub(crate) tenant_owners_file: Option<String>,
    pub(crate) policies_file: Option<String>,
    pub(crate) xkey_seed: Option<toml::Value>,
    pub(crate) credential_stream: Option<String>,
    pub(crate) admission_limit: Option<usize>,
    pub(crate) invalidation_stream_replicas: Option<usize>,
}
