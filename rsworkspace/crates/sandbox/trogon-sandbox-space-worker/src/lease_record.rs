use std::collections::BTreeMap;

use crate::SpaceId;

/// What the worker remembers about a lease it holds.
#[derive(Clone)]
pub(crate) struct LeaseRecord {
    pub(crate) sandbox_id: SpaceId,
    pub(crate) cwd: Option<String>,
    pub(crate) env: BTreeMap<String, String>,
}
