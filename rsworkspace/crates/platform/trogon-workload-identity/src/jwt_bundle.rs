use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::DecodingKey;
use prost::Message;
use serde::Deserialize;
use spiffe::TrustDomain;

use crate::JwtBundleError;
use crate::constants::{
    JWT_SVID_USE, MAX_CONTEXT_BYTES, MAX_JWKS_BYTES, MAX_JWT_AUTHORITIES, MAX_ROOTS, MIN_RSA_MODULUS_BITS,
    P521_COORDINATE_BYTES,
};
use crate::jwt_authority::{AuthorityKey, AuthorityKind, JwtAuthority};
use crate::jwt_bundles_wire::JwtBundlesWire;

/// The JWT-SVID signing keys of one trust domain, the JWT counterpart of the
/// per-trust-domain X.509 bundle. Built from the JWKS document of a Workload
/// API update: entries whose `use` is `jwt-svid`, or absent as a SPIRE agent
/// sends them, are extracted and entries with another `use` are skipped. Each
/// must carry a `kid`, and keys must be RSA of at least 2048 bits or EC on
/// P-256, P-384 or P-521 with a point on the curve.
#[derive(Debug, Clone)]
pub struct JwtBundle {
    trust_domain: TrustDomain,
    authorities: Vec<JwtAuthority>,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<RawJwk>,
}

#[derive(Deserialize)]
struct RawJwk {
    kty: String,
    #[serde(rename = "use")]
    key_use: Option<String>,
    kid: Option<String>,
    n: Option<String>,
    e: Option<String>,
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
}

impl JwtBundle {
    pub fn from_jwks(trust_domain: TrustDomain, jwks: &str) -> Result<Self, JwtBundleError> {
        if jwks.len() > MAX_JWKS_BYTES {
            return Err(JwtBundleError::TooLarge);
        }
        let document: Jwks = serde_json::from_str(jwks).map_err(|_| JwtBundleError::Malformed)?;
        let mut authorities: Vec<JwtAuthority> = Vec::new();
        for raw in document
            .keys
            .iter()
            .filter(|raw| raw.key_use.as_deref().is_none_or(|key_use| key_use == JWT_SVID_USE))
        {
            if authorities.len() == MAX_JWT_AUTHORITIES {
                return Err(JwtBundleError::TooManyAuthorities);
            }
            let authority = authority(raw)?;
            if authorities.iter().any(|known| known.key_id == authority.key_id) {
                return Err(JwtBundleError::DuplicateKeyId);
            }
            authorities.push(authority);
        }
        if authorities.is_empty() {
            return Err(JwtBundleError::NoAuthorities);
        }
        Ok(Self {
            trust_domain,
            authorities,
        })
    }

    pub fn trust_domain(&self) -> &TrustDomain {
        &self.trust_domain
    }

    pub(crate) fn authority(&self, key_id: &str) -> Option<&JwtAuthority> {
        self.authorities.iter().find(|authority| authority.key_id == key_id)
    }

    pub(crate) fn authorities(&self) -> &[JwtAuthority] {
        &self.authorities
    }
}

fn authority(raw: &RawJwk) -> Result<JwtAuthority, JwtBundleError> {
    let key_id = match raw.kid.as_deref() {
        Some(kid) if !kid.is_empty() => kid.to_owned(),
        _ => return Err(JwtBundleError::MissingKeyId),
    };
    let component = |value: &Option<String>| -> Result<Vec<u8>, JwtBundleError> {
        let value = value.as_deref().ok_or(JwtBundleError::UnsupportedKey)?;
        URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| JwtBundleError::UnsupportedKey)
    };
    let (kind, key) = match raw.kty.as_str() {
        "RSA" => {
            let modulus = component(&raw.n)?;
            let exponent = component(&raw.e)?;
            let significant = modulus.iter().skip_while(|byte| **byte == 0).count();
            if significant * 8 < MIN_RSA_MODULUS_BITS || exponent.is_empty() {
                return Err(JwtBundleError::UnsupportedKey);
            }
            (
                AuthorityKind::Rsa,
                AuthorityKey::Jose(DecodingKey::from_rsa_raw_components(&modulus, &exponent)),
            )
        }
        "EC" => {
            let (kind, width) = match raw.crv.as_deref() {
                Some("P-256") => (AuthorityKind::EcP256, 32),
                Some("P-384") => (AuthorityKind::EcP384, 48),
                Some("P-521") => (AuthorityKind::EcP521, P521_COORDINATE_BYTES),
                _ => return Err(JwtBundleError::UnsupportedKey),
            };
            let (Some(x), Some(y)) = (raw.x.as_deref(), raw.y.as_deref()) else {
                return Err(JwtBundleError::UnsupportedKey);
            };
            let (x_bytes, y_bytes) = (component(&raw.x)?, component(&raw.y)?);
            if x_bytes.len() != width || y_bytes.len() != width {
                return Err(JwtBundleError::UnsupportedKey);
            }
            let point = [&[0x04][..], &x_bytes, &y_bytes].concat();
            let key = if kind == AuthorityKind::EcP521 {
                AuthorityKey::p521(&point)
            } else {
                AuthorityKey::p256_or_p384(kind, &point, x, y)
            };
            (kind, key.ok_or(JwtBundleError::UnsupportedKey)?)
        }
        _ => return Err(JwtBundleError::UnsupportedKey),
    };
    Ok(JwtAuthority { key_id, kind, key })
}

/// JWT bundles keyed by trust domain. Bundles are never merged: a second
/// bundle for the same trust domain is refused, and a lookup only ever yields
/// the keys of the trust domain asked for.
#[derive(Debug, Clone, Default)]
pub struct JwtBundleSet {
    bundles: std::collections::BTreeMap<TrustDomain, JwtBundle>,
}

impl JwtBundleSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_bundle(mut self, bundle: JwtBundle) -> Result<Self, JwtBundleError> {
        if self.bundles.contains_key(bundle.trust_domain()) {
            return Err(JwtBundleError::DuplicateTrustDomain);
        }
        self.bundles.insert(bundle.trust_domain().clone(), bundle);
        Ok(self)
    }

    pub fn is_empty(&self) -> bool {
        self.bundles.is_empty()
    }

    /// Builds the whole set of one Workload API update. Every bundle must be
    /// valid: a single bad entry refuses the update, so a partial set is never
    /// trusted.
    pub(crate) fn from_wire(response: JwtBundlesWire) -> Result<Self, JwtBundleError> {
        if response.encoded_len() > MAX_CONTEXT_BYTES {
            return Err(JwtBundleError::TooLarge);
        }
        if response.bundles.len() > MAX_ROOTS {
            return Err(JwtBundleError::TooManyBundles);
        }
        if response.bundles.is_empty() {
            return Err(JwtBundleError::NoBundles);
        }
        let mut set = Self::new();
        for (name, jwks) in &response.bundles {
            let trust_domain = TrustDomain::new(name).map_err(|_| JwtBundleError::InvalidTrustDomain)?;
            let jwks = std::str::from_utf8(jwks).map_err(|_| JwtBundleError::Malformed)?;
            set = set.with_bundle(JwtBundle::from_jwks(trust_domain, jwks)?)?;
        }
        Ok(set)
    }

    pub fn trust_domains(&self) -> impl Iterator<Item = &TrustDomain> {
        self.bundles.keys()
    }

    pub(crate) fn bundle(&self, trust_domain: &TrustDomain) -> Option<&JwtBundle> {
        self.bundles.get(trust_domain)
    }
}
