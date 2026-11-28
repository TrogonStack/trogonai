use std::fmt;

use serde_json::{Map, Value};

use crate::argument_issue::ArgumentIssue;

/// What a model tool accepts, so a rejected call can name the field and show a call that works.
#[derive(Debug)]
pub(crate) struct ModelToolSpec {
    pub(crate) name: &'static str,
    pub(crate) fields: &'static [FieldSpec],
    pub(crate) example: &'static str,
}

#[derive(Debug)]
pub(crate) struct FieldSpec {
    pub(crate) name: &'static str,
    pub(crate) required: bool,
    pub(crate) kind: FieldKind,
    /// Field names models use instead of this one.
    pub(crate) aliases: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum FieldKind {
    Text,
    Count,
}

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

impl ModelToolSpec {
    /// The first field-level problem with these arguments.
    pub(crate) fn check(&self, arguments: &Map<String, Value>) -> Result<(), ArgumentIssue> {
        if let Some(unknown) = arguments.keys().find(|key| self.field(key).is_none()) {
            let instead = self
                .fields
                .iter()
                .find(|field| field.aliases.contains(&unknown.as_str()))
                .map(|field| field.name);
            return Err(ArgumentIssue::UnknownField {
                field: unknown.clone(),
                instead,
            });
        }
        for field in self.fields {
            match (arguments.get(field.name), field.kind) {
                (None | Some(Value::Null), _) if field.required => {
                    return Err(ArgumentIssue::MissingField(field.name));
                }
                (None | Some(Value::Null), _) | (Some(Value::String(_)), FieldKind::Text) => {}
                (Some(value), FieldKind::Count) if value.as_u64().is_some_and(|n| u32::try_from(n).is_ok()) => {}
                (Some(_), kind) => {
                    return Err(ArgumentIssue::WrongType {
                        field: field.name,
                        expected: match kind {
                            FieldKind::Text => "a string",
                            FieldKind::Count => "a non-negative integer",
                        },
                    });
                }
            }
        }
        Ok(())
    }

    fn field(&self, name: &str) -> Option<&FieldSpec> {
        self.fields.iter().find(|field| field.name == name)
    }

    pub(crate) fn write_usage(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = |required: bool| {
            self.fields
                .iter()
                .filter(move |field| field.required == required)
                .map(|field| field.name)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (required, optional) = (names(true), names(false));
        let required = if required.is_empty() {
            "no required fields"
        } else {
            required.as_str()
        };
        write!(formatter, "{} takes {required}", self.name)?;
        if !optional.is_empty() {
            write!(formatter, " (optional: {optional})")?;
        }
        write!(formatter, ".")
    }
}

#[cfg(test)]
mod tests;
