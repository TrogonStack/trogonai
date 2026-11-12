use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RosterInput {
    #[schemars(range(min = 0, max = 4_294_967_295_u64))]
    pub(crate) page_size: Option<u32>,
    pub(crate) page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SendInput {
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub(crate) recipient_peer_id: String,
    #[schemars(regex(pattern = "^[A-Za-z0-9_-]{1,128}$"))]
    pub(crate) idempotency_key: String,
    #[schemars(length(min = 1, max = 2048), description = "At most 2048 UTF-8 bytes.")]
    pub(crate) text: String,
    #[schemars(regex(pattern = "^[A-Za-z0-9_-]{1,128}$"))]
    pub(crate) reply_to_message_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StatusInput {
    #[schemars(regex(pattern = "^[A-Za-z0-9_-]{1,128}$"))]
    pub(crate) idempotency_key: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcknowledgeInput {
    #[schemars(regex(pattern = "^[A-Za-z0-9_-]{1,128}$"))]
    pub(crate) batch_id: String,
}
