//! Turning an inbound `DecideRequest` into the inputs one execution needs.
//!
//! The host decodes the request envelope and never the command inside it: only
//! the guest knows that schema, so the payload is carried through as bytes. What
//! it does parse fails loudly rather than falling back. A caller that sent an
//! idempotency key the host quietly dropped would believe it has a guarantee it
//! does not have.

use async_nats::HeaderMap;
use buffa::Message as _;
use trogon_decider_runtime::{CommandId, StreamPosition};
use trogon_decider_wasm_runtime::{CommandType, CommandTypeError};
use trogon_decider_wit::host::CommandEnvelope;
use trogonai_proto::decider::v1::DecideRequest;

use crate::constants::{CONTENT_TYPE_HEADER, PROTOBUF_CONTENT_TYPE};

/// One command as the transport delivered it.
#[derive(Debug, Clone)]
pub struct CommandRequest {
    command: CommandEnvelope,
    command_id: Option<CommandId>,
    expected_revision: Option<StreamPosition>,
}

impl CommandRequest {
    /// Reads a delivery as the `Decide` endpoint's request message.
    ///
    /// `content_type` absent is accepted: `DeciderService` declares
    /// `CONTENT_TYPE_PROTOBUF`, so nothing is ambiguous about not restating it.
    /// A *different* content type is rejected, because that caller believes it
    /// is speaking an encoding this endpoint does not accept.
    pub fn parse(payload: &[u8], headers: Option<&HeaderMap>) -> Result<Self, CommandRequestError> {
        if let Some(content_type) = headers.and_then(|headers| headers.get(CONTENT_TYPE_HEADER))
            && content_type.as_str() != PROTOBUF_CONTENT_TYPE
        {
            return Err(CommandRequestError::UnsupportedContentType {
                content_type: content_type.as_str().to_owned(),
            });
        }

        let request = DecideRequest::decode_from_slice(payload).map_err(|error| CommandRequestError::Undecodable {
            reason: error.to_string(),
        })?;

        let command = request.command.into_option().ok_or(CommandRequestError::NoCommand)?;
        let command_type =
            CommandType::new(command.type_url.clone()).map_err(|source| CommandRequestError::CommandType {
                type_url: command.type_url.clone(),
                source,
            })?;

        let command_id = request
            .command_id
            .map(|value| {
                value
                    .parse()
                    .map(CommandId::new)
                    .map_err(|source| CommandRequestError::CommandId { value, source })
            })
            .transpose()?;

        let expected_revision = request
            .expected_revision
            .map(|value| StreamPosition::try_new(value).map_err(|_| CommandRequestError::ExpectedRevisionZero))
            .transpose()?;

        Ok(Self {
            command: CommandEnvelope {
                type_: command_type.as_str().to_owned(),
                payload: command.value.into(),
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

/// Why an inbound message is not a command this host can run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandRequestError {
    /// The caller declared an encoding this endpoint does not accept.
    #[error("content type '{content_type}' is not '{PROTOBUF_CONTENT_TYPE}'")]
    UnsupportedContentType { content_type: String },
    /// The payload is not a `DecideRequest`.
    ///
    /// The decode failure is carried as its rendered form: it is a diagnostic
    /// for the caller that sent the bytes, and nothing in the host branches on
    /// it.
    #[error("payload is not a DecideRequest: {reason}")]
    Undecodable { reason: String },
    /// The request carries no command at all.
    #[error("DecideRequest carries no command")]
    NoCommand,
    /// The command's type URL does not name a usable command type.
    #[error("command type url '{type_url}' is not a valid command type: {source}")]
    CommandType {
        type_url: String,
        #[source]
        source: CommandTypeError,
    },
    /// The command id is present but is not a UUID.
    #[error("command_id '{value}' is not a uuid: {source}")]
    CommandId {
        value: String,
        #[source]
        source: uuid::Error,
    },
    /// The expected revision is present and zero.
    ///
    /// Zero would mean "I expect no events", which is the module's
    /// `WritePrecondition::NoStream` to declare rather than a revision for a
    /// caller to assert. Accepting it would let a caller express a guard the
    /// module never agreed to.
    #[error("expected_revision is zero; an empty stream is the module's no_stream precondition")]
    ExpectedRevisionZero,
}

#[cfg(test)]
mod tests;
