/// Why a JWT-SVID was refused. One variant per reason; each has a stable
/// reason code for logs (see [`JwtSvidError::code`]). The codes are an
/// operator contract and must not be renamed. No variant carries token or key
/// material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JwtSvidError {
    #[error("the mutual-TLS peer evidence of the hop has expired")]
    HopExpired,
    #[error("no JWT bundle set has been delivered by the Workload API yet")]
    BundlesNotDelivered,
    #[error("the Workload API JWT bundle stream has been lost for longer than the staleness bound")]
    BundlesStale,
    #[error("token exceeds the size bound")]
    TokenTooLarge,
    #[error("token uses the JWS JSON serialization")]
    JsonSerialization,
    #[error("token is not a JWS compact serialization of three segments")]
    NotCompactSerialization,
    #[error("token segment is not canonical base64url")]
    MalformedSegment,
    #[error("header is not a JSON object or has an invalid kid")]
    MalformedHeader,
    #[error("header carries a parameter other than alg, typ and kid")]
    UnexpectedHeader,
    #[error("header has no alg")]
    MissingAlgorithm,
    #[error("alg is not an allowed JWT-SVID algorithm")]
    AlgorithmNotAllowed,
    #[error("typ is neither JWT nor JOSE")]
    InvalidType,
    #[error("claims are not a JSON object or a claim has the wrong type")]
    MalformedClaims,
    #[error("sub is missing or is not a canonical workload SPIFFE ID")]
    InvalidSubject,
    #[error("no JWT bundle for the trust domain of sub")]
    UnknownTrustDomain,
    #[error("kid names no key in the trust domain's JWT bundle")]
    UnknownKeyId,
    #[error("no key in the trust domain's JWT bundle fits alg")]
    KeyAlgorithmMismatch,
    #[error("signature does not verify")]
    InvalidSignature,
    #[error("aud is missing")]
    MissingAudience,
    #[error("aud is not the expected audience")]
    AudienceMismatch,
    #[error("aud names more than one audience")]
    MultipleAudiences,
    #[error("exp is missing")]
    MissingExpiry,
    #[error("token has expired")]
    Expired,
    #[error("token is not yet valid")]
    NotYetValid,
    #[error("token expires further ahead than the lifetime ceiling")]
    LifetimeTooLong,
}

impl JwtSvidError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::HopExpired => "hop_expired",
            Self::BundlesNotDelivered => "bundles_not_delivered",
            Self::BundlesStale => "bundles_stale",
            Self::TokenTooLarge => "token_too_large",
            Self::JsonSerialization => "json_serialization",
            Self::NotCompactSerialization => "not_compact_serialization",
            Self::MalformedSegment => "malformed_segment",
            Self::MalformedHeader => "malformed_header",
            Self::UnexpectedHeader => "unexpected_header",
            Self::MissingAlgorithm => "missing_algorithm",
            Self::AlgorithmNotAllowed => "algorithm_not_allowed",
            Self::InvalidType => "invalid_type",
            Self::MalformedClaims => "malformed_claims",
            Self::InvalidSubject => "invalid_subject",
            Self::UnknownTrustDomain => "unknown_trust_domain",
            Self::UnknownKeyId => "unknown_key_id",
            Self::KeyAlgorithmMismatch => "key_algorithm_mismatch",
            Self::InvalidSignature => "invalid_signature",
            Self::MissingAudience => "missing_audience",
            Self::AudienceMismatch => "audience_mismatch",
            Self::MultipleAudiences => "multiple_audiences",
            Self::MissingExpiry => "missing_expiry",
            Self::Expired => "expired",
            Self::NotYetValid => "not_yet_valid",
            Self::LifetimeTooLong => "lifetime_too_long",
        }
    }
}
