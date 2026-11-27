use serde::{Deserialize, Serialize};

use crate::{AddressDenialClass, UnsupportedSigv4Shape};

/// Why the egress proxy denied a connection.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressViolationReason {
    PolicyDenied,
    GssEncryptionUnsupported,
    PostgresTlsDeclined,
    DestinationAddressDenied(AddressDenialClass),
    SecretSentinelOffHost,
    SecretCompressedResponse,
    SniDestinationMismatch,
    SniResolutionFailed,
    DeciderDenied,
    DeciderTimedOut,
    DeciderUnavailable,
    DeciderMalformedReply,
    DeciderOverloaded,
    SecretSigv4Unsupported(UnsupportedSigv4Shape),
    SecretSigv4Unsignable,
    SecretJwtMalformed,
    SecretJwtClaimsUnmatched,
    OpaqueTunnelRefused,
    RequestDenied,
    TunnelLimitReached,
    MalformedHost,
    PolicyUnavailable,
}
