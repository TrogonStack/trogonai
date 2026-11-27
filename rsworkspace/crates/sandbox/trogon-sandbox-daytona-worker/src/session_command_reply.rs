use serde::Deserialize;

/// What starting a session command answers with.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionCommandReply {
    #[serde(default)]
    pub(crate) cmd_id: Option<String>,
}
