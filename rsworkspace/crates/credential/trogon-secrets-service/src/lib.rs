#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Release core of the secrets service.
//!
//! [`CredentialRelease`] decides whether a runtime service may receive one
//! credential and, when it may, seals the value to the requester's xkey with
//! the service's xkey as the authenticated sender (ADR#0083 Decision 1). It
//! does no I/O of its own: the tenant's owners, the release policies, the
//! credential's state, the secret store, the audit trail, the sealer, and the
//! clock are all ports, so the binary that wires them is the only place that
//! connects to a bus or to OpenBao.
//!
//! [`JetStreamCredentialStates`] is the stream-backed [`CredentialStateRead`]:
//! each read folds one credential's subject of the credential event stream up
//! to the event it ended on when the read started, with no snapshot in
//! between, so a revocation committed before the read is never missed.
//!
//! [`WriterReleaseAudit`] is the [`ReleaseAudit`] on the platform audit writer:
//! a release is a must-audit `CredentialUse` of kind release, acknowledged only
//! once stored, and a denial is a deny decision on the secrets release surface
//! that stands whether or not its write is acknowledged. Both carry the
//! request's correlation id, with the runtime service as the caller.
//!
//! Every denial answers [`CredentialReleaseError::Denied`] whatever the reason, so a
//! requester cannot tell an unknown tenant from a refusing policy. The
//! [`DenialReason`] goes to the audit record and the logs only.
//!
//! [`ReleaseServer`] serves the release core on the bus as
//! `SecretExchangeService.ResolveForRelease`. It names the caller from the
//! subject alone, as a [`ReleaseCaller`], before it decodes the payload, and a
//! subject that names no caller is denied like any other refusal. The payload
//! becomes a [`ReleaseCall`], and a release still running at
//! [`RELEASE_DEADLINE`] answers `UNAVAILABLE`.
//!
//! # Features
//!
//! - `test-support`: adds `InMemoryCredentialStates` and
//!   `RecordingReleaseAudit`, in-process ports for tests that drive the release
//!   core without a credential stream or an audit writer. Off by default;
//!   enable it only from `[dev-dependencies]`.

mod admission_limit;
mod admission_limit_error;
mod constants;
mod credential_release;
mod credential_release_error;
mod credential_state_read;
mod credential_state_read_error;
mod denial_reason;
mod denial_record;
#[cfg(test)]
mod fixtures;
#[cfg(any(test, feature = "test-support"))]
mod in_memory_credential_states;
mod jetstream_credential_states;
mod owner_holding;
#[cfg(any(test, feature = "test-support"))]
mod recording_release_audit;
mod refusal;
mod release_answer;
mod release_audit;
mod release_audit_error;
mod release_audit_record_error;
mod release_audit_records;
mod release_call;
mod release_call_error;
mod release_caller;
mod release_caller_error;
mod release_credential;
mod release_destination;
mod release_fault_error;
mod release_policy_source;
mod release_record;
mod release_reply;
mod release_request;
mod release_server;
mod release_server_error;
mod requester_xkey;
#[cfg(test)]
mod scripted_release;
mod seal_error;
mod sealed_release;
mod sealed_value;
mod sealer;
mod static_release_policies;
mod static_tenant_owners;
mod tenant_account;
mod tenant_owners;
mod tenant_owners_file_error;
mod tenant_owners_file_wire;
mod throttled_answer;
mod toml_syntax;
mod writer_release_audit;
mod xkey_sealer;

pub use admission_limit::AdmissionLimit;
pub use admission_limit_error::AdmissionLimitError;
pub use constants::{
    RELEASE_DEADLINE, RELEASE_SUBSCRIPTION, RELEASE_TTL_BOUND, THROTTLED_RETRY_AFTER, UNBUNDLED_GENERATION,
};
pub use credential_release::CredentialRelease;
pub use credential_release_error::CredentialReleaseError;
pub use credential_state_read::CredentialStateRead;
pub use credential_state_read_error::CredentialStateReadError;
pub use denial_reason::DenialReason;
pub use denial_record::DenialRecord;
#[cfg(any(test, feature = "test-support"))]
pub use in_memory_credential_states::InMemoryCredentialStates;
pub use jetstream_credential_states::JetStreamCredentialStates;
pub use owner_holding::OwnerHolding;
#[cfg(any(test, feature = "test-support"))]
pub use recording_release_audit::RecordingReleaseAudit;
pub use release_answer::ReleaseAnswer;
pub use release_audit::ReleaseAudit;
pub use release_audit_error::ReleaseAuditError;
pub use release_audit_record_error::ReleaseAuditRecordError;
pub use release_call::ReleaseCall;
pub use release_call_error::ReleaseCallError;
pub use release_caller::ReleaseCaller;
pub use release_caller_error::ReleaseCallerError;
pub use release_credential::ReleaseCredential;
pub use release_destination::ReleaseDestination;
pub use release_policy_source::ReleasePolicySource;
pub use release_record::ReleaseRecord;
pub use release_request::ReleaseRequest;
pub use release_server::ReleaseServer;
pub use release_server_error::ReleaseServerError;
pub use requester_xkey::{RequesterXkey, RequesterXkeyError};
pub use seal_error::SealError;
pub use sealed_release::SealedRelease;
pub use sealed_value::{SealedValue, SealedValueError};
pub use sealer::Sealer;
pub use static_release_policies::StaticReleasePolicies;
pub use static_tenant_owners::StaticTenantOwners;
pub use tenant_account::{TenantAccount, TenantAccountError};
pub use tenant_owners::TenantOwners;
pub use tenant_owners_file_error::TenantOwnersFileError;
pub use toml_syntax::TomlSyntax;
pub use writer_release_audit::WriterReleaseAudit;
pub use xkey_sealer::{XkeySealer, XkeySealerError};
