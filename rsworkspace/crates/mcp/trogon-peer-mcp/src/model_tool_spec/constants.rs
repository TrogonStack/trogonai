use super::{FieldKind, FieldSpec, ModelToolSpec};

pub(crate) const ROSTER: ModelToolSpec = ModelToolSpec {
    name: "peer_roster",
    fields: &[
        FieldSpec {
            name: "page_size",
            required: false,
            kind: FieldKind::Count,
            aliases: &["limit"],
        },
        FieldSpec {
            name: "page_token",
            required: false,
            kind: FieldKind::Text,
            aliases: &["cursor", "token"],
        },
    ],
    example: "{}",
};

pub(crate) const SEND: ModelToolSpec = ModelToolSpec {
    name: "peer_send",
    fields: &[
        FieldSpec {
            name: "recipient_peer_id",
            required: true,
            kind: FieldKind::Text,
            aliases: &[
                "to",
                "recipient",
                "recipient_id",
                "receiver",
                "peer",
                "peer_id",
                "target",
            ],
        },
        FieldSpec {
            name: "idempotency_key",
            required: true,
            kind: FieldKind::Text,
            aliases: &["key", "id", "message_id", "request_id"],
        },
        FieldSpec {
            name: "text",
            required: true,
            kind: FieldKind::Text,
            aliases: &["content", "message", "body", "msg"],
        },
        FieldSpec {
            name: "reply_to_message_id",
            required: false,
            kind: FieldKind::Text,
            aliases: &["reply_to", "in_reply_to"],
        },
    ],
    example: r#"{"recipient_peer_id":"<64-hex peerId from peer_roster>","idempotency_key":"ask-api-1","text":"your message"}"#,
};

pub(crate) const STATUS: ModelToolSpec = ModelToolSpec {
    name: "peer_status",
    fields: &[FieldSpec {
        name: "idempotency_key",
        required: true,
        kind: FieldKind::Text,
        aliases: &["key", "id", "message_id"],
    }],
    example: r#"{"idempotency_key":"ask-api-1"}"#,
};

pub(crate) const ACKNOWLEDGE: ModelToolSpec = ModelToolSpec {
    name: "peer_acknowledge_batch",
    fields: &[FieldSpec {
        name: "batch_id",
        required: true,
        kind: FieldKind::Text,
        aliases: &["batch", "id"],
    }],
    example: r#"{"batch_id":"<batch_id from the delivered peer messages>"}"#,
};
