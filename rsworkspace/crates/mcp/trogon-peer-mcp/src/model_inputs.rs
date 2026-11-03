use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RosterInput {
    pub(crate) page_size: Option<u32>,
    pub(crate) page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SendInput {
    pub(crate) recipient_peer_id: String,
    pub(crate) idempotency_key: String,
    pub(crate) text: String,
    pub(crate) reply_to_message_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StatusInput {
    pub(crate) idempotency_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcknowledgeInput {
    pub(crate) batch_id: String,
}
