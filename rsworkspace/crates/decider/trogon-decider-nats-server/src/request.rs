//! Turning an inbound NATS message into the inputs one execution needs.
//!
//! The host never decodes a command payload: only the guest knows the schema.
//! What it does parse is the envelope around it, and every parse here fails
//! loudly rather than falling back. A caller that sent an idempotency key the
//! host quietly dropped would believe it has a guarantee it does not have.

use async_nats::HeaderMap;
use trogon_decider_runtime::{CommandId, StreamPosition};
use trogon_decider_wasm_runtime::CommandType;
use trogon_decider_wit::host::CommandEnvelope;

use crate::constants::{
    CONTENT_TYPE_HEADER, PROTOBUF_CONTENT_TYPE, TROGON_COMMAND_ID_HEADER, TROGON_EXPECTED_REVISION_HEADER,
};

/// One command as the transport delivered it.
#[derive(Debug, Clone)]
pub struct CommandRequest {
    command: CommandEnvelope,
    command_id: Option<CommandId>,
    expected_revision: Option<StreamPosition>,
}

impl CommandRequest {
    /// Assembles a request from the subject-derived type, the payload, and the headers.
    ///
    /// `content_type` absent is accepted: protobuf is the only encoding this
    /// binding defines, so nothing is ambiguous about not saying so. A
    /// *different* content type is rejected, because that caller believes it is
    /// speaking an encoding the host does not implement.
    pub fn parse(
        command_type: &CommandType,
        payload: Vec<u8>,
        headers: Option<&HeaderMap>,
    ) -> Result<Self, CommandRequestError> {
        if let Some(content_type) = headers.and_then(|headers| headers.get(CONTENT_TYPE_HEADER))
            && content_type.as_str() != PROTOBUF_CONTENT_TYPE
        {
            return Err(CommandRequestError::UnsupportedContentType {
                content_type: content_type.as_str().to_owned(),
            });
        }

        let command_id = headers
            .and_then(|headers| headers.get(TROGON_COMMAND_ID_HEADER))
            .map(|value| {
                value
                    .as_str()
                    .parse()
                    .map(CommandId::new)
                    .map_err(|source| CommandRequestError::CommandId {
                        value: value.as_str().to_owned(),
                        source,
                    })
            })
            .transpose()?;

        let expected_revision = headers
            .and_then(|headers| headers.get(TROGON_EXPECTED_REVISION_HEADER))
            .map(|value| parse_expected_revision(value.as_str()))
            .transpose()?;

        Ok(Self {
            command: CommandEnvelope {
                type_: command_type.as_str().to_owned(),
                payload,
            },
            command_id,
            expected_revision,
        })
    }

    /// The envelope handed to the guest.
    pub const fn command(&self) -> &CommandEnvelope {
        &self.command
    }

    /// The caller's command identity, if it sent one.
    pub const fn command_id(&self) -> Option<CommandId> {
        self.command_id
    }

    /// The revision the caller believes it is acting on, if it sent one.
    pub const fn expected_revision(&self) -> Option<StreamPosition> {
        self.expected_revision
    }
}

fn parse_expected_revision(value: &str) -> Result<StreamPosition, CommandRequestError> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| CommandRequestError::ExpectedRevisionNotANumber {
            value: value.to_owned(),
        })?;

    StreamPosition::try_new(parsed).map_err(|_| CommandRequestError::ExpectedRevisionZero)
}

/// Why an inbound message is not a command this host can run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandRequestError {
    /// The caller declared an encoding this binding does not define.
    #[error("content type '{content_type}' is not '{PROTOBUF_CONTENT_TYPE}'")]
    UnsupportedContentType { content_type: String },
    /// The command id header is present but is not a UUID.
    #[error("{TROGON_COMMAND_ID_HEADER} '{value}' is not a uuid: {source}")]
    CommandId {
        value: String,
        #[source]
        source: uuid::Error,
    },
    /// The expected revision header is present but is not a number.
    #[error("{TROGON_EXPECTED_REVISION_HEADER} '{value}' is not an unsigned integer")]
    ExpectedRevisionNotANumber { value: String },
    /// The expected revision header is present and zero.
    ///
    /// Zero would mean "I expect no events", which is the module's
    /// `WritePrecondition::NoStream` to declare rather than a revision for a
    /// caller to assert. Accepting it would let a caller express a guard the
    /// module never agreed to.
    #[error("{TROGON_EXPECTED_REVISION_HEADER} is zero; an empty stream is the module's no_stream precondition")]
    ExpectedRevisionZero,
}

#[cfg(test)]
mod tests;
