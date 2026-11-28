use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RosterInput {
    #[schemars(
        range(min = 0, max = 4_294_967_295_u64),
        description = "Optional; omit it for the default page."
    )]
    pub(crate) page_size: Option<u32>,
    #[schemars(description = "Optional; only a nextPageToken from a previous peer_roster result.")]
    pub(crate) page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SendInput {
    #[schemars(
        regex(pattern = "^[0-9a-f]{64}$"),
        description = "Exact 64-hex peerId of the recipient from peer_roster or a message's sender_peer_id; not a display name, not your own peerId."
    )]
    pub(crate) recipient_peer_id: String,
    #[schemars(
        regex(pattern = "^[A-Za-z0-9_-]{1,128}$"),
        description = "Your own key for this message, new for each message; reuse it only to retry the same message."
    )]
    pub(crate) idempotency_key: String,
    #[schemars(length(min = 1, max = 2048), description = "Message text, at most 2048 UTF-8 bytes.")]
    pub(crate) text: String,
    #[schemars(
        regex(pattern = "^[A-Za-z0-9_-]{1,128}$"),
        description = "Optional message_id of the delivered peer message you are answering."
    )]
    pub(crate) reply_to_message_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StatusInput {
    #[schemars(
        regex(pattern = "^[A-Za-z0-9_-]{1,128}$"),
        description = "The idempotency_key you passed to peer_send."
    )]
    pub(crate) idempotency_key: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcknowledgeInput {
    #[schemars(
        regex(pattern = "^[A-Za-z0-9_-]{1,128}$"),
        description = "batch_id from the delivered peer messages."
    )]
    pub(crate) batch_id: String,
}
