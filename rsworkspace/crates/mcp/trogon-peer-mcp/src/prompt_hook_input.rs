use serde::Deserialize;
use trogon_peer::NativeSessionId;

use crate::PeerMcpError;
use crate::prompt_id::PromptId;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PromptHookInput {
    session_id: String,
    prompt_id: Option<String>,
    #[serde(rename = "hook_event_name")]
    _hook_event_name: PromptHookEvent,
}

#[derive(Deserialize)]
enum PromptHookEvent {
    UserPromptSubmit,
}

impl PromptHookInput {
    pub(crate) fn into_identity(self) -> Result<(NativeSessionId, Option<PromptId>), PeerMcpError> {
        let session_id = NativeSessionId::new(self.session_id).map_err(PeerMcpError::InvalidHostIdentity)?;
        let prompt_id = self
            .prompt_id
            .filter(|value| !value.is_empty())
            .map(PromptId::new)
            .transpose()
            .map_err(PeerMcpError::InvalidHostIdentity)?;
        Ok((session_id, prompt_id))
    }
}
