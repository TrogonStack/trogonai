use trogon_sandbox_protocol::AttemptLedgerError;

use super::{E2bProviderError, LeaseLifetimeError};
use crate::api::{ApiError, E2bConfigError, NativeAccessTokenError};
use crate::assignments::AssignmentsError;
use crate::envd::EnvdTargetError;
use crate::exec::ExecError;
use crate::lease_cleanup::LeaseCleanupError;

/// Why targeted dispatch, recovery or cleanup refused an assignment.
#[derive(Debug, thiserror::Error)]
pub enum E2bDispatchError {
    #[error(transparent)]
    Provider(#[from] E2bProviderError),
    #[error(transparent)]
    Assignments(#[from] AssignmentsError),
    #[error(transparent)]
    Ledger(#[from] AttemptLedgerError),
    #[error(transparent)]
    Exec(#[from] ExecError),
    #[error(transparent)]
    Cleanup(#[from] LeaseCleanupError),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Lifetime(#[from] LeaseLifetimeError),
    #[error(transparent)]
    Config(#[from] E2bConfigError),
    #[error(transparent)]
    Token(#[from] NativeAccessTokenError),
    #[error(transparent)]
    Envd(#[from] EnvdTargetError),
    #[error("dispatch ledger was already configured")]
    LedgerAlreadyConfigured,
    #[error("targeted dispatch is not configured")]
    NotConfigured,
    #[error("targeted dispatch does not match this worker assignment")]
    TargetMismatch,
    #[error("targeted cleanup is not configured")]
    CleanupNotConfigured,
    #[error("targeted cleanup belongs to another worker")]
    CleanupForeignWorker,
    #[error("dispatch ledger lacks durable assignment ownership")]
    LedgerLacksAssignment,
    #[error("native recovery lacks an independently recovered dispatch tenant witness")]
    RecoveryLacksTenantWitness,
    #[error("native recovery tenant differs from the retained dispatch tenant")]
    RecoveryTenantMismatch,
    #[error("retained native receipt changed assignment")]
    ReceiptChangedAssignment,
    #[error("unsettled native allocation must retain nonready custody")]
    UnsettledNotRetained,
    #[error("ready assignment lacks a native receipt")]
    ReadyLacksReceipt,
    #[error("ready assignment lacks durable ownership")]
    ReadyLacksOwnership,
    #[error("native adoption does not match durable assignment ownership")]
    AdoptionMismatch,
    #[error("native adoption lacks recovery metadata")]
    AdoptionLacksRecovery,
    #[error("exact native identity or running state could not be confirmed")]
    NativeIdentityUnconfirmed,
    #[error("native recovery has no access token")]
    RecoveryLacksToken,
    #[error("native recovery would replace an already held lease")]
    RecoveryWouldReplace,
    #[error("ready dispatch lacks assignment ownership")]
    ReadyDispatchLacksAssignment,
    #[error("ready dispatch changed tenant or native identity")]
    ReadyDispatchChanged,
    #[error("allocation began without a retained native receipt")]
    CleanupWithoutReceipt,
    #[error("native cleanup lacks durable assignment ownership")]
    CleanupLacksOwnership,
    #[error("native cleanup tenant or resource authority changed")]
    CleanupAuthorityChanged,
    #[error("native cleanup does not match the held lease")]
    CleanupDoesNotMatchHeld,
}
