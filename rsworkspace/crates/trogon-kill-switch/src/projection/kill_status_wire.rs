use serde::{Deserialize, Serialize};

use crate::{KillReason, KillReasonError, KillSwitchState, OccurredAt, OccurredAtError};

/// Wire shape of a projected [`KillSwitchState`] as stored in the
/// `KILL_SWITCH_STATE` KV bucket.
///
/// A boundary type, not the domain type: it is what `serde_json` reads and
/// writes, and it is fallible to convert into [`KillSwitchState`] (an unknown
/// `reason` string or malformed `since` timestamp must be rejected, not
/// silently coerced). Only [`KillSwitchState`] is ever handed to callers.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
enum KillStatusWire {
    #[serde(rename = "alive")]
    Alive,
    #[serde(rename = "killed")]
    Killed { reason: String, since: String },
}

#[derive(Debug, thiserror::Error)]
pub enum KillStatusWireError {
    #[error("failed to serialize kill status: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to deserialize kill status: {0}")]
    Deserialize(#[source] serde_json::Error),
    #[error("invalid kill reason in projected status: {0}")]
    KillReason(#[source] KillReasonError),
    #[error("invalid timestamp in projected status: {0}")]
    OccurredAt(#[source] OccurredAtError),
}

pub fn encode_kill_status(state: &KillSwitchState) -> Result<Vec<u8>, KillStatusWireError> {
    let wire = match state {
        KillSwitchState::Alive => KillStatusWire::Alive,
        KillSwitchState::Killed { reason, since } => KillStatusWire::Killed {
            reason: reason.as_str().to_owned(),
            since: since.to_rfc3339().map_err(KillStatusWireError::OccurredAt)?,
        },
    };
    serde_json::to_vec(&wire).map_err(KillStatusWireError::Serialize)
}

pub fn decode_kill_status(bytes: &[u8]) -> Result<KillSwitchState, KillStatusWireError> {
    let wire: KillStatusWire = serde_json::from_slice(bytes).map_err(KillStatusWireError::Deserialize)?;
    match wire {
        KillStatusWire::Alive => Ok(KillSwitchState::Alive),
        KillStatusWire::Killed { reason, since } => Ok(KillSwitchState::Killed {
            reason: KillReason::parse(&reason).map_err(KillStatusWireError::KillReason)?,
            since: OccurredAt::parse_rfc3339(&since).map_err(KillStatusWireError::OccurredAt)?,
        }),
    }
}

#[cfg(test)]
mod tests;
