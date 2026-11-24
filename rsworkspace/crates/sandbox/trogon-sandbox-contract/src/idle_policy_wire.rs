use serde::Deserialize;

use crate::IdleStage;

/// The JSON shape of an [`IdlePolicy`](crate::IdlePolicy) before its stage rules are checked.
#[derive(Deserialize)]
pub(crate) struct IdlePolicyWire {
    pub(crate) stages: Vec<IdleStage>,
}
