use std::time::SystemTime;

use buffa::{Message as _, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogon_credential_aggregate::{InvalidationReason, credential_project_token};
use trogon_credential_domain::CredentialKey;
use trogon_decider_runtime::StreamPosition;
use trogonai_proto::secrets::exchange::v1alpha1::{CredentialAddress, ReleaseInvalidated, ReleaseInvalidationReason};

use super::constants::INVALIDATION_SUBJECT_INFIX;
use super::{InvalidationSourceError, TenantAccount};

/// One release invalidation notice, ready to publish to
/// [`INVALIDATION_STREAM`].
///
/// The subject starts with the account of the tenant holding the credential's
/// owner, so only that account can import the notice (ADR#0081 Decision 3).
/// The `Nats-Msg-Id` names the credential event behind the notice by the
/// stream it was read from and its sequence there, so the stream keeps one
/// notice however often that event is delivered, and services reading
/// differently named credential streams never collide. The payload
/// is an encoded `ReleaseInvalidated`, which has no field that could hold a
/// value (ADR#0023 Decision 3).
///
/// [`INVALIDATION_STREAM`]: crate::INVALIDATION_STREAM
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidationNotice {
    subject: String,
    message_id: String,
    payload: Vec<u8>,
}

impl InvalidationNotice {
    /// The notice that `tenant`'s holders of `credential` drop it for
    /// `reason`, from the credential event the stream named `source_stream`
    /// appended at `occurred_at` and `source_sequence`.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidationSourceError::NotAppendedAfterEpoch`] when
    /// `occurred_at` is not after the Unix epoch, which the contract refuses,
    /// and [`InvalidationSourceError::NoSubject`] when the owner has no subject
    /// token.
    pub(crate) fn new(
        tenant: &TenantAccount,
        credential: &CredentialKey,
        reason: InvalidationReason,
        source_stream: &str,
        occurred_at: SystemTime,
        source_sequence: StreamPosition,
    ) -> Result<Self, InvalidationSourceError> {
        let sequence = source_sequence.as_u64();
        if occurred_at <= SystemTime::UNIX_EPOCH {
            return Err(InvalidationSourceError::NotAppendedAfterEpoch { sequence });
        }
        let owner = credential.owner_id();
        let token = credential_project_token(owner).map_err(InvalidationSourceError::NoSubject)?;
        let payload = ReleaseInvalidated {
            credential: MessageField::some(CredentialAddress {
                credential_id: credential.credential_id().to_string(),
                owner_id: owner.as_str().to_owned(),
            }),
            reason: wire_reason(reason).into(),
            occurred_at: MessageField::some(Timestamp::from(occurred_at)),
            source_sequence: sequence,
        };
        Ok(Self {
            subject: format!(
                "{INVALIDATION_SUBJECT_INFIX}.{tenant}.{}.{}",
                token.as_str(),
                credential.credential_id()
            ),
            message_id: format!("{source_stream}:{sequence}"),
            payload: payload.encode_to_vec(),
        })
    }

    /// The subject to publish on:
    /// `secrets.invalidation.v1.{tenant}.{owner_token}.{credential_id}`.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The `Nats-Msg-Id` to publish with, `{source_stream}:{source_sequence}`,
    /// `CREDENTIAL_EVENTS:{source_sequence}` on the default credential stream.
    pub fn message_id(&self) -> &str {
        &self.message_id
    }

    /// The encoded `ReleaseInvalidated`.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

fn wire_reason(reason: InvalidationReason) -> ReleaseInvalidationReason {
    match reason {
        InvalidationReason::Rotated => ReleaseInvalidationReason::RELEASE_INVALIDATION_REASON_ROTATED,
        InvalidationReason::Revoked => ReleaseInvalidationReason::RELEASE_INVALIDATION_REASON_REVOKED,
        InvalidationReason::Destroyed => ReleaseInvalidationReason::RELEASE_INVALIDATION_REASON_DESTROYED,
    }
}

#[cfg(test)]
mod tests;
