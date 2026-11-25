use jsonwebtoken::Algorithm;

/// The `alg` values a JWT-SVID may carry: RFC 7518 sections 3.3 to 3.5. Parsing
/// is exact and case sensitive; `none`, HMAC, EdDSA and the ML-DSA names a later
/// revision of the JWT-SVID specification lists do not parse. All nine
/// algorithms verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JwtSvidAlgorithm {
    Rs256,
    Rs384,
    Rs512,
    Es256,
    Es384,
    Es512,
    Ps256,
    Ps384,
    Ps512,
}

impl JwtSvidAlgorithm {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "RS256" => Self::Rs256,
            "RS384" => Self::Rs384,
            "RS512" => Self::Rs512,
            "ES256" => Self::Es256,
            "ES384" => Self::Es384,
            "ES512" => Self::Es512,
            "PS256" => Self::Ps256,
            "PS384" => Self::Ps384,
            "PS512" => Self::Ps512,
            _ => return None,
        })
    }

    pub(crate) fn is_rsa(self) -> bool {
        matches!(
            self,
            Self::Rs256 | Self::Rs384 | Self::Rs512 | Self::Ps256 | Self::Ps384 | Self::Ps512
        )
    }

    /// The `jsonwebtoken` algorithm, `None` for ES512: `jsonwebtoken` has no
    /// P-521 support, so ES512 is verified by `JwtAuthority::verifies`.
    pub(crate) fn backend(self) -> Option<Algorithm> {
        Some(match self {
            Self::Rs256 => Algorithm::RS256,
            Self::Rs384 => Algorithm::RS384,
            Self::Rs512 => Algorithm::RS512,
            Self::Es256 => Algorithm::ES256,
            Self::Es384 => Algorithm::ES384,
            Self::Es512 => return None,
            Self::Ps256 => Algorithm::PS256,
            Self::Ps384 => Algorithm::PS384,
            Self::Ps512 => Algorithm::PS512,
        })
    }
}
