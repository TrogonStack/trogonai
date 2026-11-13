use async_nats::jetstream::message::StreamMessage;
use async_nats::jetstream::stream::LastRawMessageErrorKind;
use trogon_credential_aggregate::{credential_event_subject, evolve_stream, initial_state};
use trogon_credential_domain::CredentialKey;
use trogon_decider_nats::record_stream_message;
use trogon_decider_runtime::EventDecodeOutcome;
use trogon_nats::jetstream::{JetStreamFirstRawMessageBySubject, JetStreamLastRawMessageBySubject};
use trogonai_proto::gateway::credentials::state_v1::CredentialStateSnapshot;
use trogonai_proto::gateway::credentials::v1;

use super::{CredentialStateRead, CredentialStateReadError};

/// Credential states folded from the credential stream on every read.
///
/// A read fixes its bound at the last event the credential's subject holds
/// when the read starts, then folds that subject from its first event up to
/// the bound. There is no snapshot to lag behind the stream, so a revocation
/// committed before the read is always part of the fold, and an event appended
/// once the read has started is left for the next read.
///
/// Every event is read from the stream leader, one at a time along the
/// subject. A credential's history is a handful of events, so the read costs
/// one round trip per event plus one for the bound.
///
/// The read fails closed. An event this build cannot decode, of a type it does
/// not know, or that does not follow from the state before it fails the read,
/// and so does a bound that vanished while the read was under way. A partial
/// fold is never an answer: the events it missed may be the revocation.
#[derive(Clone, Debug)]
pub struct JetStreamCredentialStates<S> {
    stream: S,
}

impl<S> JetStreamCredentialStates<S> {
    /// Reads credential states from `stream`, the credential event stream.
    pub fn new(stream: S) -> Self {
        Self { stream }
    }
}

impl<S> CredentialStateRead for JetStreamCredentialStates<S>
where
    S: JetStreamLastRawMessageBySubject + JetStreamFirstRawMessageBySubject + Sync,
{
    async fn read(
        &self,
        credential: &CredentialKey,
    ) -> Result<Option<CredentialStateSnapshot>, CredentialStateReadError> {
        let subject = credential_event_subject(credential).map_err(unavailable)?;
        let subject = subject.as_str();
        let bound = match self.stream.get_last_raw_message_by_subject(subject).await {
            Ok(last) => last.sequence,
            Err(error) if error.kind() == LastRawMessageErrorKind::NoMessageFound => return Ok(None),
            Err(error) => return Err(unavailable(error)),
        };

        let mut state = initial_state();
        let mut from = 1;
        loop {
            let message = self
                .stream
                .get_first_raw_message_by_subject(subject, from)
                .await
                .map_err(unavailable)?;
            let sequence = message.sequence;
            if sequence > bound {
                return Err(CredentialStateReadError::Unavailable {
                    message: BOUND_VANISHED.to_owned(),
                });
            }
            state = fold(credential, subject, state, message)?;
            if sequence == bound {
                return Ok(Some(state));
            }
            from = sequence + 1;
        }
    }
}

/// Why a read that found its bound gone answers nothing.
const BOUND_VANISHED: &str = "the last event of the credential's subject was removed during the read";

/// Folds one event read from `credential`'s subject into `state`.
fn fold(
    credential: &CredentialKey,
    subject: &str,
    state: CredentialStateSnapshot,
    message: StreamMessage,
) -> Result<CredentialStateSnapshot, CredentialStateReadError> {
    let event = record_stream_message(message, subject).map_err(undecodable)?;
    match event.decode::<v1::CredentialEvent>().map_err(undecodable)? {
        EventDecodeOutcome::Decoded(event) => evolve_stream(credential, state, &event).map_err(undecodable),
        // The gateway's projection skips an event type it does not own. A
        // release cannot: the unknown event may be the one that ends it.
        EventDecodeOutcome::Skipped => Err(CredentialStateReadError::Undecodable {
            message: format!("event type '{}' is not a credential event", event.event.r#type),
        }),
    }
}

fn unavailable(error: impl std::error::Error) -> CredentialStateReadError {
    CredentialStateReadError::Unavailable {
        message: error.to_string(),
    }
}

fn undecodable(error: impl std::error::Error) -> CredentialStateReadError {
    CredentialStateReadError::Undecodable {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests;
