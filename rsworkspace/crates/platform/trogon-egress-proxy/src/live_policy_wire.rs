use std::num::NonZeroU64;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Envelope {
    pub tenant: String,
    pub project: String,
    pub incarnation: NonZeroU64,
    pub generation: NonZeroU64,
    pub issued_at: u64,
    pub valid_until: u64,
    pub policy: String,
}
