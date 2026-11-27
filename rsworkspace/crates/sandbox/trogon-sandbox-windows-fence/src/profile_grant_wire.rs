use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub(crate) struct ProfileGrantWire {
    pub(crate) profile: String,
    pub(crate) containers: u64,
    pub(crate) granted: u64,
    pub(crate) files: u64,
    pub(crate) denied: u64,
    pub(crate) placeholders: u64,
    pub(crate) writes: u64,
    pub(crate) unchanged: u64,
    pub(crate) removed: u64,
    pub(crate) skipped: Option<Value>,
    pub(crate) not_granted: Option<Value>,
    pub(crate) self_closed: Option<Value>,
    pub(crate) unverifiable: Option<Value>,
    pub(crate) unprotected: Option<Value>,
    pub(crate) stale_allow: Option<Value>,
    pub(crate) forgotten_roots: Option<Value>,
    pub(crate) elapsed_ms: u64,
}
