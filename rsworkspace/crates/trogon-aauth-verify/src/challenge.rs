//! Mint `aa-resource+jwt` challenge tokens (the 401 response body).

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use trogon_identity_types::aauth::{DWK_RESOURCE, MissionRef, TYP_RESOURCE};

use crate::time_source::TimeSource;

#[derive(Debug, thiserror::Error)]
pub enum ChallengeError {
    /// Wraps the typed `jsonwebtoken` encode error so the source chain is
    /// preserved instead of being flattened to a String.
    #[error("encode: {0}")]
    Encode(#[from] jsonwebtoken::errors::Error),
    #[error("ttl overflowed i64 when added to iat ({iat} + {ttl_secs})")]
    TtlOverflow { iat: i64, ttl_secs: i64 },
}

/// Inputs to mint a resource challenge token.
pub struct ResourceChallenge<'a> {
    pub iss: &'a str,
    pub aud_ps: &'a str,
    pub agent: &'a str,
    pub agent_jkt: &'a str,
    pub scope: &'a str,
    pub ttl_secs: i64,
    pub kid: &'a str,
    pub jti: &'a str,
    pub mission: Option<MissionRef>,
}

/// Mints the resource challenge JWT.
pub struct ChallengeMinter<C: TimeSource> {
    pub signing_key: EncodingKey,
    pub alg: Algorithm,
    pub clock: C,
}

impl<C: TimeSource> ChallengeMinter<C> {
    pub fn new(signing_key: EncodingKey, alg: Algorithm, clock: C) -> Self {
        Self {
            signing_key,
            alg,
            clock,
        }
    }

    pub fn mint(&self, c: &ResourceChallenge<'_>) -> Result<String, ChallengeError> {
        let iat = self.clock.now();
        let exp = iat.checked_add(c.ttl_secs).ok_or(ChallengeError::TtlOverflow {
            iat,
            ttl_secs: c.ttl_secs,
        })?;

        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            aud: &'a str,
            jti: &'a str,
            iat: i64,
            exp: i64,
            dwk: &'a str,
            agent: &'a str,
            agent_jkt: &'a str,
            scope: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            mission: Option<&'a MissionRef>,
        }

        let mut header = Header::new(self.alg);
        header.typ = Some(TYP_RESOURCE.into());
        header.kid = Some(c.kid.into());

        encode(
            &header,
            &Claims {
                iss: c.iss,
                aud: c.aud_ps,
                jti: c.jti,
                iat,
                exp,
                dwk: DWK_RESOURCE,
                agent: c.agent,
                agent_jkt: c.agent_jkt,
                scope: c.scope,
                mission: c.mission.as_ref(),
            },
            &self.signing_key,
        )
        .map_err(ChallengeError::from)
    }
}

/// One-shot helper for callers that don't keep a long-lived minter.
pub fn mint_resource_jwt(
    signing_key: &EncodingKey,
    alg: Algorithm,
    iat: i64,
    c: &ResourceChallenge<'_>,
) -> Result<String, ChallengeError> {
    struct Fixed(i64);
    impl TimeSource for Fixed {
        fn now(&self) -> i64 {
            self.0
        }
    }
    let minter = ChallengeMinter::new(signing_key.clone(), alg, Fixed(iat));
    minter.mint(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::EncodingKey;

    fn challenge<'a>(jti: &'a str, ttl_secs: i64) -> ResourceChallenge<'a> {
        ResourceChallenge {
            iss: "ps.example",
            aud_ps: "ps.example",
            agent: "agent-1",
            agent_jkt: "abc",
            scope: "read",
            ttl_secs,
            kid: "kid-1",
            jti,
            mission: None,
        }
    }

    #[test]
    fn mint_succeeds_with_normal_inputs() {
        // Pin a real ES256 key so this exercises the happy path (proving
        // the Encode source-wrapping path doesn't fire for valid input).
        let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
        let key = EncodingKey::from_ec_pem(pem).expect("test key");
        let minter = ChallengeMinter::new(key, Algorithm::ES256, super::tests::FixedClock(1000));
        let token = minter.mint(&challenge("jti-1", 60)).expect("mint succeeds");
        assert!(token.split('.').count() == 3, "got {token}");
    }

    #[test]
    fn mint_returns_ttl_overflow_at_i64_max() {
        let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
        let key = EncodingKey::from_ec_pem(pem).expect("test key");
        let minter = ChallengeMinter::new(key, Algorithm::ES256, super::tests::FixedClock(i64::MAX));
        let err = minter.mint(&challenge("jti-2", 1)).unwrap_err();
        assert!(
            matches!(err, ChallengeError::TtlOverflow { iat, ttl_secs } if iat == i64::MAX && ttl_secs == 1),
            "got {err:?}"
        );
    }

    pub(super) struct FixedClock(pub i64);
    impl TimeSource for FixedClock {
        fn now(&self) -> i64 {
            self.0
        }
    }
}
