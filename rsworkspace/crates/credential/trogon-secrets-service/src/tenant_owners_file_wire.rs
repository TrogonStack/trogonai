use std::collections::BTreeMap;

use serde::Deserialize;

/// The tenant owners file as written: each tenant's NATS account key with the
/// credential owners it holds.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TenantOwnersFileWire {
    pub(crate) tenants: BTreeMap<String, Vec<String>>,
}
