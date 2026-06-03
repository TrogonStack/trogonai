//! Mint `aa-resource+jwt` challenge tokens (the 401 response body).

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use trogon_identity_types::aauth::{DWK_RESOURCE, MissionRef, TYP_RESOURCE};

use crate::time_source::TimeSource;

#[derive(Debug, thiserror::Error)]
pub enum ChallengeError {
    #[error("encode: {0}")]
    Encode(String),
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
        Self { signing_key, alg, clock }
    }

    pub fn mint(&self, c: &ResourceChallenge<'_>) -> Result<String, ChallengeError> {
        let iat = self.clock.now();
        let exp = iat + c.ttl_secs;

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
        .map_err(|e| ChallengeError::Encode(e.to_string()))
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
