//! JWT-SVID verification.
//!
//! X.509-SVID with mutual TLS stays the default workload credential. A
//! JWT-SVID is for the hops where an L7 intermediary terminates TLS and a
//! certificate cannot be presented end to end. A JWT-SVID is a bearer token:
//! whoever holds it can replay it until it expires, so it must never be
//! forwarded to another service; each hop obtains a token for its own audience.
//!
//! [`JwtSvidVerifier::verify`] is the only way to obtain a
//! [`VerifiedJwtSvid`], and it applies every rule below, in this order:
//!
//! The bundles come from a [`JwtBundleSource`] fed by the Workload API
//! `FetchJWTBundles` stream, so a rotated authority is trusted as soon as it is
//! delivered and a removed one stops being trusted at the next update. The
//! verifier reads the current set on every call and fails closed, with
//! `bundles_not_delivered` or `bundles_stale`, before it looks at the token
//! when the source has never delivered a set or the stream has been lost for
//! longer than [`JWT_BUNDLE_MAX_STALENESS`](crate::constants::JWT_BUNDLE_MAX_STALENESS).
//!
//! 0. The token arrived over a [`ConfidentialHop`]: an authenticated mutual-TLS
//!    peer whose evidence has not expired, or a local Unix domain socket peer.
//!    `verify` cannot be called without one, and the hop kind is recorded in the
//!    [`VerifiedJwtSvid`].
//! 1. JWS compact serialization only: exactly three base64url segments, never
//!    the JSON serialization, bounded in size.
//! 2. Header: `alg` is one of RS256/384/512, ES256/384/512, PS256/384/512
//!    (`none`, HMAC and EdDSA are refused) before any key is looked up; `typ`,
//!    when present, is `JWT` or `JOSE`; no parameter other than `alg`, `typ` and
//!    `kid` is allowed.
//! 3. `sub` is a canonical SPIFFE ID of a workload (non-empty path, lowercase
//!    trust domain). Its trust domain selects the JWT bundle; a trust domain
//!    without a configured bundle is refused, and no other trust domain's keys
//!    are ever tried.
//! 4. The signature verifies under a key of that bundle: the one named by
//!    `kid`, or, when the token has no `kid`, any key of that bundle whose type
//!    fits `alg`.
//! 5. `aud` is present and names exactly one audience, the verifier's single
//!    expected audience. A token scoped to several receivers is refused even
//!    when one of them is this verifier, because any of those receivers could
//!    replay it to the others.
//! 6. `exp` is present and in the future (within a bounded leeway), and no
//!    further ahead than the lifetime ceiling; `nbf`, when present, has passed.
//!
//! All nine algorithms verify. `jsonwebtoken` has no P-521 support, so ES512
//! is verified with aws-lc-rs (`ECDSA_P521_SHA512_FIXED`, the 132-byte `R || S`
//! of RFC 7518 section 3.4); the key lookup, claims and bundle checks are
//! shared by every algorithm.
//!
//! Refusals are logged at `warn` with a stable `reason` code and nothing else.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::constants::{
    DEFAULT_JWT_SVID_LEEWAY, DEFAULT_JWT_SVID_MAX_LIFETIME, MAX_JWT_SVID_LEEWAY, MAX_JWT_SVID_LIFETIME_CEILING,
};
use crate::jwt_authority::JwtAuthority;
use crate::jwt_token::ParsedToken;
use crate::{
    ConfidentialHop, JwtAudience, JwtBundle, JwtBundleSource, JwtBundleSourceError, JwtClock, JwtSvidError,
    JwtVerifierError, SystemJwtClock, VerifiedJwtSvid, WorkloadId, telemetry,
};

#[derive(Debug, Clone)]
pub struct JwtSvidVerifier {
    audience: JwtAudience,
    bundles: JwtBundleSource,
    max_lifetime: Duration,
    leeway: Duration,
    clock: Arc<dyn JwtClock>,
}

impl JwtSvidVerifier {
    pub fn new(audience: JwtAudience, bundles: JwtBundleSource) -> Self {
        Self {
            audience,
            bundles,
            max_lifetime: DEFAULT_JWT_SVID_MAX_LIFETIME,
            leeway: DEFAULT_JWT_SVID_LEEWAY,
            clock: Arc::new(SystemJwtClock),
        }
    }

    /// Ceiling on how far ahead of now `exp` may be. Default 300 seconds.
    pub fn with_max_lifetime(mut self, max_lifetime: Duration) -> Result<Self, JwtVerifierError> {
        if max_lifetime.is_zero() || max_lifetime > MAX_JWT_SVID_LIFETIME_CEILING {
            return Err(JwtVerifierError::LifetimeCeilingInvalid);
        }
        self.max_lifetime = max_lifetime;
        Ok(self)
    }

    /// Tolerated clock skew for `exp` and `nbf`. Default 30 seconds, at most 60.
    pub fn with_leeway(mut self, leeway: Duration) -> Result<Self, JwtVerifierError> {
        if leeway > MAX_JWT_SVID_LEEWAY {
            return Err(JwtVerifierError::LeewayInvalid);
        }
        self.leeway = leeway;
        Ok(self)
    }

    pub fn with_clock(mut self, clock: Arc<dyn JwtClock>) -> Self {
        self.clock = clock;
        self
    }

    /// Verifies `token`, which must have arrived over `hop`.
    pub fn verify(&self, token: &str, hop: &ConfidentialHop) -> Result<VerifiedJwtSvid, JwtSvidError> {
        self.check(token, hop)
            .inspect_err(|error| telemetry::jwt_svid_refused(error.code()))
    }

    fn check(&self, token: &str, hop: &ConfidentialHop) -> Result<VerifiedJwtSvid, JwtSvidError> {
        if hop
            .expires_at()
            .is_some_and(|expires_at| expires_at <= self.clock.now())
        {
            return Err(JwtSvidError::HopExpired);
        }
        let bundles = self.bundles.current().map_err(|error| match error {
            JwtBundleSourceError::NeverDelivered => JwtSvidError::BundlesNotDelivered,
            JwtBundleSourceError::Stale => JwtSvidError::BundlesStale,
        })?;
        let parsed = ParsedToken::parse(token)?;

        let subject = match parsed.claims.get("sub") {
            Some(Value::String(subject)) => subject,
            _ => return Err(JwtSvidError::InvalidSubject),
        };
        let workload_id = WorkloadId::new(subject).map_err(|_| JwtSvidError::InvalidSubject)?;
        let bundle = bundles
            .bundle(workload_id.as_spiffe_id().trust_domain())
            .ok_or(JwtSvidError::UnknownTrustDomain)?;

        let candidates = candidates(bundle, &parsed)?;
        let verified = candidates
            .iter()
            .any(|authority| authority.verifies(parsed.algorithm, parsed.signing_input, parsed.signature));
        if !verified {
            return Err(JwtSvidError::InvalidSignature);
        }

        self.check_audience(parsed.claims.get("aud"))?;
        let expires_at = self.check_times(&parsed)?;
        Ok(VerifiedJwtSvid::new(
            workload_id,
            self.audience.as_str().to_owned(),
            expires_at,
            hop.kind(),
        ))
    }

    fn check_audience(&self, audience: Option<&Value>) -> Result<(), JwtSvidError> {
        let matched = match audience {
            None | Some(Value::Null) => return Err(JwtSvidError::MissingAudience),
            Some(Value::String(single)) => single == self.audience.as_str(),
            Some(Value::Array(values)) => {
                if values.iter().any(|value| !value.is_string()) {
                    return Err(JwtSvidError::MalformedClaims);
                }
                match values.as_slice() {
                    [] => return Err(JwtSvidError::MissingAudience),
                    [Value::String(single)] => single == self.audience.as_str(),
                    _ => return Err(JwtSvidError::MultipleAudiences),
                }
            }
            Some(_) => return Err(JwtSvidError::MalformedClaims),
        };
        if matched {
            Ok(())
        } else {
            Err(JwtSvidError::AudienceMismatch)
        }
    }

    fn check_times(&self, parsed: &ParsedToken<'_>) -> Result<SystemTime, JwtSvidError> {
        let now = self
            .clock
            .now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let leeway = self.leeway.as_secs();
        let exp = match parsed.claims.get("exp") {
            None | Some(Value::Null) => return Err(JwtSvidError::MissingExpiry),
            Some(value) => numeric_date(value)?,
        };
        if exp.saturating_add(leeway) <= now {
            return Err(JwtSvidError::Expired);
        }
        if exp.saturating_sub(now) > self.max_lifetime.as_secs() {
            return Err(JwtSvidError::LifetimeTooLong);
        }
        if let Some(value) = parsed.claims.get("nbf")
            && numeric_date(value)? > now.saturating_add(leeway)
        {
            return Err(JwtSvidError::NotYetValid);
        }
        Ok(UNIX_EPOCH + Duration::from_secs(exp))
    }
}

/// Keys of `bundle` that may have signed the token. Never reaches outside it.
fn candidates<'a>(bundle: &'a JwtBundle, parsed: &ParsedToken<'_>) -> Result<Vec<&'a JwtAuthority>, JwtSvidError> {
    match &parsed.key_id {
        Some(key_id) => {
            let authority = bundle.authority(key_id).ok_or(JwtSvidError::UnknownKeyId)?;
            if authority.kind.accepts(parsed.algorithm) {
                Ok(vec![authority])
            } else {
                Err(JwtSvidError::KeyAlgorithmMismatch)
            }
        }
        None => {
            let fitting: Vec<_> = bundle
                .authorities()
                .iter()
                .filter(|authority| authority.kind.accepts(parsed.algorithm))
                .collect();
            if fitting.is_empty() {
                Err(JwtSvidError::KeyAlgorithmMismatch)
            } else {
                Ok(fitting)
            }
        }
    }
}

/// A NumericDate in whole seconds, rounded down so a fractional `exp` is never
/// extended.
fn numeric_date(value: &Value) -> Result<u64, JwtSvidError> {
    let Value::Number(number) = value else {
        return Err(JwtSvidError::MalformedClaims);
    };
    if let Some(seconds) = number.as_u64() {
        return Ok(seconds);
    }
    match number.as_f64() {
        Some(seconds) if seconds.is_finite() && seconds >= 0.0 => Ok(seconds.floor() as u64),
        _ => Err(JwtSvidError::MalformedClaims),
    }
}
