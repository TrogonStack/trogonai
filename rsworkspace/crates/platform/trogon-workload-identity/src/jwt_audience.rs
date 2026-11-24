use spiffe::TrustDomain;

use crate::JwtVerifierError;
use crate::constants::MAX_JWT_AUDIENCE_BYTES;

/// The one audience a verifier identifies with: an absolute URI naming a single
/// service, such as `spiffe://prod.example.com/reports`.
///
/// A bare trust domain (`example.org`, `spiffe://example.org`,
/// `spiffe://example.org/`), a bare word such as `production` and anything
/// without a scheme and a non-empty authority cannot be built, so a verifier can
/// never be configured with an audience broad enough to be replayed elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtAudience(String);

impl JwtAudience {
    pub fn new(value: &str) -> Result<Self, JwtVerifierError> {
        if value.is_empty()
            || value.len() > MAX_JWT_AUDIENCE_BYTES
            || value.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(JwtVerifierError::AudienceInvalid);
        }
        let Some((scheme, rest)) = value.split_once("://") else {
            return Err(if TrustDomain::new(value.to_ascii_lowercase()).is_ok() {
                JwtVerifierError::AudienceIsTrustDomain
            } else {
                JwtVerifierError::AudienceNotSpecific
            });
        };
        if !is_scheme(scheme) {
            return Err(JwtVerifierError::AudienceNotSpecific);
        }
        let authority_and_path = rest.trim_end_matches('/');
        if scheme.eq_ignore_ascii_case("spiffe") {
            if !authority_and_path.contains('/') {
                return Err(JwtVerifierError::AudienceIsTrustDomain);
            }
        } else if authority_and_path.is_empty() || authority_and_path.starts_with('/') {
            return Err(JwtVerifierError::AudienceNotSpecific);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_scheme(scheme: &str) -> bool {
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}
