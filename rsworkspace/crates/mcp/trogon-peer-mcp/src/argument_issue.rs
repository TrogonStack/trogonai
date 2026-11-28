use std::fmt;

use trogon_peer::PeerValueError;

use crate::model_text::quoted;

/// One concrete problem with a model tool call, phrased so the model can fix it.
#[derive(Debug)]
pub(crate) enum ArgumentIssue {
    UnknownField {
        field: String,
        instead: Option<&'static str>,
    },
    MissingField(&'static str),
    WrongType {
        field: &'static str,
        expected: &'static str,
    },
    InvalidValue {
        field: &'static str,
        rule: &'static str,
        source: PeerValueError,
    },
    MalformedRecipient {
        characters: usize,
        source: PeerValueError,
    },
}

impl ArgumentIssue {
    pub(crate) fn source(&self) -> Option<&PeerValueError> {
        match self {
            Self::InvalidValue { source, .. } | Self::MalformedRecipient { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl fmt::Display for ArgumentIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownField {
                field,
                instead: Some(instead),
            } => write!(formatter, "unknown field {}; use `{instead}` instead.", quoted(field)),
            Self::UnknownField { field, instead: None } => write!(formatter, "unknown field {}.", quoted(field)),
            Self::MissingField(field) => write!(formatter, "`{field}` is required."),
            Self::WrongType { field, expected } => write!(formatter, "`{field}` must be {expected}."),
            Self::InvalidValue { field, rule, source } => write!(formatter, "`{field}` {rule} ({source})."),
            Self::MalformedRecipient { characters, .. } => write!(
                formatter,
                "`recipient_peer_id` must be exactly 64 lowercase hex characters, copied from peer_roster or from \
                 a delivered message's sender_peer_id; the value sent has {characters} characters."
            ),
        }
    }
}
