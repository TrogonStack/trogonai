use serde::Deserialize;
use trogon_peer::NativeSessionId;

use crate::detach_hook_event::DetachHookEvent;
use crate::peer_mcp_error::PeerMcpError;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DetachHookInput {
    #[schemars(length(min = 1, max = 256))]
    session_id: String,
    #[serde(rename = "hook_event_name")]
    _hook_event_name: DetachHookEvent,
}

impl DetachHookInput {
    pub(crate) fn into_session(self) -> Result<NativeSessionId, PeerMcpError> {
        NativeSessionId::new(self.session_id).map_err(PeerMcpError::InvalidHostIdentity)
    }
}
