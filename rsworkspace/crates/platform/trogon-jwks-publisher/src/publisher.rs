//! Well-known JWKS discovery publishing.
//!
//! Per "Metadata Documents" (RFC 8615), an Agent Provider (or Resource /
//! Person / Access server) publishes its public keys at
//! `{iss}/.well-known/{dwk}` where `dwk` is one of the four registered
//! discoverable-well-known filenames. This module exposes an axum router a
//! host service mounts at its own root so `GET /.well-known/{dwk}` resolves
//! against a configured set of `JwkSet`s.

use std::collections::{HashMap, HashSet};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine as _;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk,
    JwkSet, PublicKeyUse,
};
use p256::ecdsa::SigningKey;
use p256::pkcs8::DecodePrivateKey;
use trogon_identity_types::aauth::{DWK_ACCESS, DWK_AGENT, DWK_PERSON, DWK_RESOURCE};

use crate::constants::JWK_SET_CONTENT_TYPE;

/// `Cache-Control: max-age=<n>` value, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheMaxAge(u64);

impl CacheMaxAge {
    #[must_use]
    pub fn new(seconds: u64) -> Self {
        Self(seconds)
    }

    #[must_use]
    pub fn as_secs(self) -> u64 {
        self.0
    }

    fn header_value(self) -> String {
        format!("max-age={}", self.0)
    }
}

/// Errors building a [`JwksPublisherConfig`].
#[derive(Debug, thiserror::Error)]
pub enum PublisherError {
    #[error(
        "unknown dwk filename {0:?}; must be one of aauth-agent.json, aauth-resource.json, aauth-person.json, aauth-access.json"
    )]
    UnknownDwk(String),
    #[error("dwk filename {0:?} was registered more than once")]
    DuplicateDwk(String),
    #[error("dwk {dwk:?} cannot be published: {source}")]
    Unpublishable {
        dwk: String,
        #[source]
        source: UnpublishableJwkSet,
    },
    #[error("invalid EC PKCS8 PEM for kid {kid:?}: {source}")]
    InvalidPem {
        kid: String,
        #[source]
        source: p256::pkcs8::Error,
    },
}

/// The four dwk filenames the draft registers under "Metadata Documents".
fn known_dwk_filenames() -> [&'static str; 4] {
    [DWK_AGENT, DWK_RESOURCE, DWK_PERSON, DWK_ACCESS]
}

fn is_known_dwk(dwk: &str) -> bool {
    known_dwk_filenames().contains(&dwk)
}

/// Why a [`JwkSet`] cannot be published as it stands. Names no dwk: which
/// document a set was registered under is the registrar's context, not the
/// set's own.
#[derive(Debug, thiserror::Error)]
pub enum UnpublishableJwkSet {
    #[error("key id {kid:?} is published more than once")]
    DuplicateKeyId { kid: String },
    #[error("{keys} keys are published and at least one omits `kid`; only a single-key set may omit it")]
    MissingKeyId { keys: usize },
}

/// A [`JwkSet`] every consumer can actually select a key from.
///
/// A published set is selected from by `kid`: [`JwkSet::find`] matches the JWT
/// header's `kid` against `common.key_id`, and `trogon-aauth-verify`'s
/// `pick_jwk` does the same, falling back to the sole compatible key only when
/// the set holds exactly one.
///
/// That makes two shapes unusable. Keys sharing a `kid` resolve silently to
/// whichever copy is enumerated first, so a rotation that reuses the previous
/// `kid` keeps every consumer pinned to the retired key. A key with no `kid`
/// cannot be selected at all once the set holds more than one, which is exactly
/// the state a rotation overlap creates. Both fail at the consumer, remotely,
/// with nothing to see on this side, so they are refused at startup instead --
/// the same reason the dwk filename is checked at registration rather than left
/// to surface as a 404.
///
/// Holding that as a type rather than a check run on the way past means a set
/// reaching [`JwksPublisherConfig`] has already been through it, and an
/// unselectable one has no way to be represented there at all. An empty set
/// (`{"keys":[]}`) is publishable: it is a legitimate discovery state, not a
/// misconfiguration.
#[derive(Debug, Clone)]
pub struct PublishableJwkSet(JwkSet);

impl TryFrom<JwkSet> for PublishableJwkSet {
    type Error = UnpublishableJwkSet;

    fn try_from(set: JwkSet) -> Result<Self, Self::Error> {
        let multi_key = set.keys.len() > 1;
        let mut seen: HashSet<&str> = HashSet::new();
        for jwk in &set.keys {
            match jwk.common.key_id.as_deref() {
                Some(kid) => {
                    if !seen.insert(kid) {
                        return Err(UnpublishableJwkSet::DuplicateKeyId { kid: kid.to_owned() });
                    }
                }
                None if multi_key => {
                    return Err(UnpublishableJwkSet::MissingKeyId { keys: set.keys.len() });
                }
                None => {}
            }
        }
        Ok(Self(set))
    }
}

impl PublishableJwkSet {
    /// The underlying set, for serialization. Borrowed rather than owned so
    /// the validated value cannot be taken apart and put back together with
    /// the invariant broken.
    #[must_use]
    pub fn as_jwk_set(&self) -> &JwkSet {
        &self.0
    }
}

/// Build a public EC P-256 JWK from a PKCS8 PEM private key, mirroring
/// `trogon-aauth-sdk`'s `public_jwk` helper but returning a typed
/// `jsonwebtoken::jwk::Jwk` (this crate already depends on `jsonwebtoken` for
/// the provider module, so publisher config can build `JwkSet`s directly).
pub fn jwk_from_ec_pkcs8_pem(pem: &str, kid: &str) -> Result<Jwk, PublisherError> {
    let signing_key = SigningKey::from_pkcs8_pem(pem).map_err(|source| PublisherError::InvalidPem {
        kid: kid.to_string(),
        source,
    })?;
    let verifying = signing_key.verifying_key();
    let point = verifying.to_encoded_point(false);
    // An uncompressed SEC1 point from a valid P-256 verifying key always
    // carries both coordinates; encode() only omits them for the compressed
    // form, which we did not request above.
    let x = point.x().map(base64_url).unwrap_or_default();
    let y = point.y().map(base64_url).unwrap_or_default();

    Ok(Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_id: Some(kid.to_string()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: EllipticCurve::P256,
            x,
            y,
        }),
    })
}

fn base64_url(bytes: impl AsRef<[u8]>) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Builder for [`JwksPublisherConfig`]. Validates dwk filenames and rejects
/// duplicate registrations up front so misconfiguration fails at startup
/// rather than as a silent 404 in production.
#[derive(Debug, Default)]
pub struct JwksPublisherConfigBuilder {
    max_age: CacheMaxAge,
    entries: HashMap<String, PublishableJwkSet>,
}

impl JwksPublisherConfigBuilder {
    #[must_use]
    pub fn new(max_age: CacheMaxAge) -> Self {
        Self {
            max_age,
            entries: HashMap::new(),
        }
    }

    /// Register a pre-built `JwkSet` under a dwk filename. This is the one
    /// boundary where a raw set is converted into a [`PublishableJwkSet`];
    /// past here the configuration holds nothing else.
    pub fn with_jwk_set(mut self, dwk: impl Into<String>, set: JwkSet) -> Result<Self, PublisherError> {
        let dwk = dwk.into();
        if !is_known_dwk(&dwk) {
            return Err(PublisherError::UnknownDwk(dwk));
        }
        if self.entries.contains_key(&dwk) {
            return Err(PublisherError::DuplicateDwk(dwk));
        }
        let set = PublishableJwkSet::try_from(set).map_err(|source| PublisherError::Unpublishable {
            dwk: dwk.clone(),
            source,
        })?;
        self.entries.insert(dwk, set);
        Ok(self)
    }

    /// Register a `JwkSet` built from a single EC P-256 PKCS8 PEM key under a
    /// dwk filename, via [`jwk_from_ec_pkcs8_pem`].
    pub fn with_ec_pkcs8_pem(self, dwk: impl Into<String>, pem: &str, kid: &str) -> Result<Self, PublisherError> {
        let jwk = jwk_from_ec_pkcs8_pem(pem, kid)?;
        self.with_jwk_set(dwk, JwkSet { keys: vec![jwk] })
    }

    #[must_use]
    pub fn build(self) -> JwksPublisherConfig {
        JwksPublisherConfig {
            max_age: self.max_age,
            entries: self.entries,
        }
    }
}

/// Validated publisher configuration: dwk filename -> [`PublishableJwkSet`],
/// plus the `Cache-Control` max-age applied to every discovery response.
#[derive(Clone)]
pub struct JwksPublisherConfig {
    max_age: CacheMaxAge,
    entries: HashMap<String, PublishableJwkSet>,
}

/// Build the `GET /.well-known/{dwk}` discovery router. Mount this into a
/// host service's own `axum::Router` (e.g. via `Router::merge`).
pub fn router(config: JwksPublisherConfig) -> Router {
    Router::new()
        .route("/.well-known/{dwk}", get(serve_dwk))
        .with_state(config)
}

/// Encodes `value` as a JWKS response body, or a `500` if serialization
/// fails. Generic (rather than inlined into `serve_dwk`, which is pinned to
/// the fixed `JwkSet` value type by axum's handler signature) so the
/// error arm -- unreachable for this module's plain-field `JwkSet` values --
/// is exercisable in isolation with a deliberately failing `Serialize` impl.
fn jwks_response(value: &impl serde::Serialize, cache_max_age: &CacheMaxAge) -> Response {
    let body = match serde_json::to_vec(value) {
        Ok(body) => body,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let mut response = body.into_response();
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(JWK_SET_CONTENT_TYPE));
    if let Ok(value) = HeaderValue::from_str(&cache_max_age.header_value()) {
        response.headers_mut().insert(header::CACHE_CONTROL, value);
    }
    response
}

async fn serve_dwk(State(config): State<JwksPublisherConfig>, Path(dwk): Path<String>) -> Response {
    let Some(set) = config.entries.get(&dwk) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    jwks_response(set.as_jwk_set(), &config.max_age)
}

#[cfg(test)]
mod tests;
