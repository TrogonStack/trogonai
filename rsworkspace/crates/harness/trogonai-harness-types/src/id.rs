//! Opaque, non-empty identifiers.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Rejected construction of an identifier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// The candidate held no characters.
    #[error("{kind} id must not be empty")]
    Empty {
        /// Identifier flavor that was being built.
        kind: &'static str,
    },
    /// The candidate was longer than this flavor of identifier may be.
    #[error("{kind} id must not exceed {limit} bytes")]
    TooLong {
        /// Identifier flavor that was being built.
        kind: &'static str,
        /// The longest this flavor may be, in bytes.
        limit: usize,
    },
    /// The candidate held something this flavor of identifier may not hold.
    #[error("{kind} id must not be {reason}")]
    NotAName {
        /// Identifier flavor that was being built.
        kind: &'static str,
        /// What is wrong with it.
        reason: &'static str,
    },
}

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident, $label:literal) => {
        string_id!($(#[$meta])* $name, $label, anything_non_empty);
    };
    ($(#[$meta:meta])* $name:ident, $label:literal, $check:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(source: D) -> Result<Self, D::Error> {
                Self::new(String::deserialize(source)?).map_err(serde::de::Error::custom)
            }
        }

        impl $name {
            /// Wraps a string this flavor of identifier accepts.
            pub fn new(raw: impl Into<String>) -> Result<Self, IdError> {
                let raw = raw.into();
                if raw.is_empty() {
                    return Err(IdError::Empty { kind: $label });
                }
                $check(&raw, $label)?;
                Ok(Self(raw))
            }

            /// Borrows the underlying text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Unwraps into the owned text.
            #[must_use]
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;

            fn try_from(raw: String) -> Result<Self, Self::Error> {
                Self::new(raw)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

/// The longest a session identifier may be.
///
/// The limit on a single filename component nearly everywhere, so an identifier
/// that fits here is a directory name that fits on disk.
pub const MAX_SESSION_ID_BYTES: usize = 255;

/// Every identifier is non-empty; this flavor asks for nothing more.
fn anything_non_empty(_raw: &str, _label: &'static str) -> Result<(), IdError> {
    Ok(())
}

/// Whether this text could be the name of one session's directory.
///
/// Alphanumeric, `.`, `_` and `-` only, which leaves out every separator on
/// every platform the runtime runs on, and neither `.` nor `..`, which name a directory
/// that already exists.
fn a_directory_could_be_called_this(raw: &str, label: &'static str) -> Result<(), IdError> {
    if raw.len() > MAX_SESSION_ID_BYTES {
        return Err(IdError::TooLong {
            kind: label,
            limit: MAX_SESSION_ID_BYTES,
        });
    }
    if raw == "." || raw == ".." {
        return Err(IdError::NotAName {
            kind: label,
            reason: "a directory that already exists",
        });
    }
    if !raw
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(IdError::NotAName {
            kind: label,
            reason: "anything but letters, digits, and `.`, `_` or `-`",
        });
    }
    Ok(())
}

string_id!(
    /// Identity of a persisted conversation.
    ///
    /// Also the name of the directory that conversation lives in, which is why
    /// it is narrower than the other identifiers here: it arrives back from a
    /// resume flag, a stored pointer, or a directory listing, and by the time it
    /// is joined onto the sessions directory nobody remembers where it came
    /// from. One that could name somewhere else cannot be built.
    SessionId,
    "session",
    a_directory_could_be_called_this
);
string_id!(
    /// Provider-assigned identity of a single tool invocation.
    ToolCallId,
    "tool call"
);
string_id!(
    /// Registered name of a tool, as advertised to the model.
    ToolName,
    "tool name"
);
string_id!(
    /// Provider-qualified model slug, such as `anthropic/claude-opus-4`.
    ModelId,
    "model"
);
string_id!(
    /// Identity of a spawned subagent.
    ///
    /// A subagent's identity names the directory its session is stored in, so
    /// it is held to the same shape a session identity is.
    SubagentId,
    "subagent",
    a_directory_could_be_called_this
);
string_id!(
    /// Identity of a configured agent mode, such as `ask` or `auto`.
    ModeId,
    "mode"
);
string_id!(
    /// Identity of a configured MCP server.
    McpServerId,
    "mcp server"
);
/// The longest a work identifier may be.
pub const MAX_WORK_ID_BYTES: usize = 128;

/// Whether this text could be a join key.
///
/// It travels in event envelopes and gets compared byte for byte against what a
/// turn already carries, so it has to be short enough to store beside every turn
/// and free of the one byte that would truncate it on its way through anything
/// that treats text as C strings.
fn a_join_key_could_be_called_this(raw: &str, label: &'static str) -> Result<(), IdError> {
    if raw.len() > MAX_WORK_ID_BYTES {
        return Err(IdError::TooLong {
            kind: label,
            limit: MAX_WORK_ID_BYTES,
        });
    }
    if raw.contains('\0') {
        return Err(IdError::NotAName {
            kind: label,
            reason: "text with a null byte in it",
        });
    }
    Ok(())
}

/// The longest a contract or check identifier may be.
pub const MAX_CONTRACT_ID_BYTES: usize = 64;

/// Whether this text could name a contract or one of its checks.
///
/// Both are shown to the model and written into traces and reports beside other
/// text, so they are kept to a short token that cannot be mistaken for prose or
/// break a line.
fn a_contract_label_could_be_called_this(raw: &str, label: &'static str) -> Result<(), IdError> {
    if raw.len() > MAX_CONTRACT_ID_BYTES {
        return Err(IdError::TooLong {
            kind: label,
            limit: MAX_CONTRACT_ID_BYTES,
        });
    }
    if !raw
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(IdError::NotAName {
            kind: label,
            reason: "anything but letters, digits, and `.`, `_` or `-`",
        });
    }
    Ok(())
}

string_id!(
    /// Identity of a task contract.
    ContractId,
    "contract",
    a_contract_label_could_be_called_this
);

string_id!(
    /// Identity of one acceptance check within a task contract.
    CheckId,
    "check",
    a_contract_label_could_be_called_this
);

string_id!(
    /// Join key tying a user turn to the child work a manager owns.
    ///
    /// Metadata only: what the model is shown for a turn stays its text and its
    /// images, and this says nothing about either.
    WorkId,
    "work",
    a_join_key_could_be_called_this
);

string_id!(
    /// The id a client was told a user message goes by.
    ///
    /// ACP version 2 names every message, and a resumed session has to name a
    /// message it replays the way it named it when the message was accepted.
    /// Kept beside the turn for that, under the same rules as [`WorkId`]: it is
    /// stored with every turn that has one.
    MessageId,
    "message",
    a_join_key_could_be_called_this
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_id_is_held_to_the_rules_of_a_join_key() {
        assert!(MessageId::new("0123456789abcdef0123456789abcdef").is_ok());
        assert!(MessageId::new("m".repeat(MAX_WORK_ID_BYTES)).is_ok());
        assert!(MessageId::new("m".repeat(MAX_WORK_ID_BYTES + 1)).is_err());
        assert!(MessageId::new("").is_err());
        assert!(MessageId::new("a\0b").is_err());
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(SessionId::new(""), Err(IdError::Empty { kind: "session" }));
    }

    #[test]
    fn a_session_that_could_name_somewhere_else_is_not_a_session() {
        for raw in [
            ".",
            "..",
            "../outside",
            "/tmp/outside",
            "nested/session",
            "nested\\session",
            "sess\0ion",
            "sessi\u{00ff}n",
        ] {
            assert!(
                matches!(SessionId::new(raw), Err(IdError::NotAName { .. })),
                "{raw:?} names somewhere else"
            );
        }
    }

    #[test]
    fn a_session_named_longer_than_a_filename_may_be_is_refused() {
        assert!(SessionId::new("a".repeat(MAX_SESSION_ID_BYTES)).is_ok());
        assert!(matches!(
            SessionId::new("a".repeat(MAX_SESSION_ID_BYTES + 1)),
            Err(IdError::TooLong { .. })
        ));
    }

    #[test]
    fn a_dot_inside_a_session_name_is_not_a_dot_directory() {
        for raw in [".hidden-session", "session..branch", "session.v3", "last"] {
            assert_eq!(SessionId::new(raw).expect("a name a directory may have").as_str(), raw);
        }
    }

    #[test]
    fn the_other_identifiers_ask_only_that_they_hold_something() {
        assert!(ToolCallId::new("call/42:x").is_ok());
    }

    #[test]
    fn a_join_key_too_long_to_store_beside_a_turn_is_refused() {
        assert!(WorkId::new("w".repeat(MAX_WORK_ID_BYTES)).is_ok());
        assert!(matches!(
            WorkId::new("w".repeat(MAX_WORK_ID_BYTES + 1)),
            Err(IdError::TooLong { .. })
        ));
    }

    #[test]
    fn a_join_key_that_would_be_cut_short_somewhere_is_refused() {
        assert!(matches!(WorkId::new("work\0hidden"), Err(IdError::NotAName { .. })));
        assert!(WorkId::new("work-\u{03bb}").is_ok(), "any other text is a key");
    }

    #[test]
    fn a_stored_identifier_is_held_to_the_same_rule_as_a_built_one() {
        assert!(serde_json::from_str::<SessionId>(r#""../outside""#).is_err());
        assert!(serde_json::from_str::<WorkId>(r#""""#).is_err());
        assert_eq!(
            serde_json::from_str::<WorkId>(r#""job-9""#).expect("a stored key"),
            WorkId::new("job-9").expect("a key")
        );
    }

    #[test]
    fn round_trips_text() {
        let id = ToolCallId::new("call_42").expect("non-empty");
        assert_eq!(id.as_str(), "call_42");
        assert_eq!(id.to_string(), "call_42");
    }
}
