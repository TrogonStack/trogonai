use serde::Deserialize;
use serde_json::Value;
use trogon_sandbox_contract::{LeaseId, ProcessName, UnixSignal};

use crate::PayloadError;
use crate::from_payload::from_payload;

#[derive(Deserialize)]
struct KillProcessPayload {
    lease_id: LeaseId,
    name: ProcessName,
    #[serde(default)]
    signal: Option<UnixSignal>,
}

/// The lease, process name and signal (default `TERM`) of `sandbox_kill_process`.
pub(crate) fn kill_process(payload: &Value) -> Result<(LeaseId, ProcessName, UnixSignal), PayloadError> {
    let kill: KillProcessPayload = from_payload(payload)?;
    Ok((kill.lease_id, kill.name, kill.signal.unwrap_or(UnixSignal::Term)))
}
