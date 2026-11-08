use std::collections::HashMap;
use std::fmt;
use std::num::ParseIntError;
use std::path::Path;
use std::sync::Arc;

use jsonwebtoken::jwk::JwkSet;
use trogon_aauth_verify::{
    CachedJwksResolver, ChallengeKey, ChallengeKeyError, ChallengeKid, ClockTolerance, ClockToleranceError,
    DEFAULT_TTL_SECS, EmptyChallengeKidError, HttpJwksResolver, HttpPopVerifier, JwksResolver, NonceRequirement,
    NonceTtl, NonceTtlError, ReplayStore, ServerNonces, SignatureFreshness, SignatureValidity, SignatureValidityError,
    StaticJwks, SystemTimeSource, TimeSource,
};
use trogon_std::env::ReadEnv;
use trogon_std::fs::ReadFile;

use super::{
    EdgeAuthMode, EdgeAuthority, EdgeAuthorityError, EmptyResourceIdentifierError, PopEdgeGuard, ProofPolicy,
    ResourceIdentifier,
};
use crate::constants::{
    DEV_UNENFORCED_VALUES, ENV_AAUTH_AUTHORITY, ENV_AAUTH_CHALLENGE_KEY_PATH, ENV_AAUTH_CHALLENGE_KID,
    ENV_AAUTH_DEV_UNENFORCED, ENV_AAUTH_JWKS_ALLOWED_ISSUERS, ENV_AAUTH_JWKS_DISCOVERY, ENV_AAUTH_JWKS_PATH,
    ENV_AAUTH_JWKS_TTL_SECS, ENV_AAUTH_LEEWAY_SECS, ENV_AAUTH_MAX_SKEW_SECS, ENV_AAUTH_MODE, ENV_AAUTH_NONCE,
    ENV_AAUTH_NONCE_TTL_SECS, ENV_AAUTH_RESOURCE_ISS, ENV_AAUTH_SIGNATURE_VALIDITY_SECS,
};

/// How the HTTP edge treats AAuth proof of possession. Enforce is the default,
/// and anything below it needs [`ENV_AAUTH_DEV_UNENFORCED`] as well, so a
/// deployment cannot drop to it by setting one variable.
#[derive(Debug)]
pub enum EdgeAuthConfig {
    Off,
    Proof(ProofSettings),
}

impl EdgeAuthConfig {
    /// # Errors
    ///
    /// [`EdgeAuthConfigError`] naming the variable to fix, whenever the mode
    /// asks for something the environment does not supply.
    pub fn from_env(env: &impl ReadEnv, fs: &impl ReadFile) -> Result<Self, EdgeAuthConfigError> {
        let mode = mode_from_env(env)?;
        let policy = match (mode, DevUnenforced::from_env(env)?) {
            (EdgeAuthMode::Enforce, _) => ProofPolicy::Enforce,
            (EdgeAuthMode::Shadow | EdgeAuthMode::Off, DevUnenforced::Unset) => {
                return Err(EdgeAuthConfigError::UnenforcedWithoutDevFlag(mode));
            }
            (EdgeAuthMode::Shadow, DevUnenforced::Granted) => ProofPolicy::Shadow,
            (EdgeAuthMode::Off, DevUnenforced::Granted) => return Ok(Self::Off),
        };
        ProofSettings::from_env(policy, env, fs).map(Self::Proof)
    }

    #[must_use]
    pub fn mode(&self) -> EdgeAuthMode {
        match self {
            Self::Off => EdgeAuthMode::Off,
            Self::Proof(settings) => settings.policy.mode(),
        }
    }
}

/// What the verifying modes check a proof against.
pub struct ProofSettings {
    policy: ProofPolicy,
    jwks: Arc<dyn JwksResolver>,
    resource: ResourceIdentifier,
    authority: EdgeAuthority,
    nonces: Option<ServerNonces>,
    freshness: SignatureFreshness,
    leeway: ClockTolerance,
}

impl ProofSettings {
    fn from_env(policy: ProofPolicy, env: &impl ReadEnv, fs: &impl ReadFile) -> Result<Self, EdgeAuthConfigError> {
        let resource = ResourceIdentifier::new(required_var(env, ENV_AAUTH_RESOURCE_ISS)?)
            .map_err(EdgeAuthConfigError::InvalidResource)?;
        let authority = EdgeAuthority::new(required_var(env, ENV_AAUTH_AUTHORITY)?)
            .map_err(EdgeAuthConfigError::InvalidAuthority)?;
        let jwks = jwks_from_env(env, fs)?;
        let nonces = nonces_from_env(env, fs)?;
        let validity = optional_secs(env, ENV_AAUTH_SIGNATURE_VALIDITY_SECS)?
            .map(SignatureValidity::new)
            .transpose()
            .map_err(EdgeAuthConfigError::InvalidSignatureValidity)?
            .unwrap_or_default();
        let skew = clock_tolerance(env, ENV_AAUTH_MAX_SKEW_SECS)?.unwrap_or_default();
        let leeway = clock_tolerance(env, ENV_AAUTH_LEEWAY_SECS)?.unwrap_or(skew);
        Ok(Self {
            policy,
            jwks,
            resource,
            authority,
            nonces,
            freshness: SignatureFreshness::new(validity, skew),
            leeway,
        })
    }

    #[must_use]
    pub fn policy(&self) -> ProofPolicy {
        self.policy
    }

    #[must_use]
    pub fn resource(&self) -> &ResourceIdentifier {
        &self.resource
    }

    #[must_use]
    pub fn authority(&self) -> &EdgeAuthority {
        &self.authority
    }

    #[must_use]
    pub fn freshness(&self) -> SignatureFreshness {
        self.freshness
    }

    #[must_use]
    pub fn leeway(&self) -> ClockTolerance {
        self.leeway
    }

    #[must_use]
    pub fn nonces(&self) -> Option<&ServerNonces> {
        self.nonces.as_ref()
    }

    /// The guard that verifies against these settings, recording each
    /// admitted `jti` in `replay`.
    #[must_use]
    pub fn guard(self, replay: Arc<dyn ReplayStore>) -> PopEdgeGuard {
        let clock: Arc<dyn TimeSource> = Arc::new(SystemTimeSource);
        let verifier = HttpPopVerifier::new(self.jwks, clock, replay, self.resource.as_str())
            .with_freshness(self.freshness)
            .with_token_leeway(self.leeway);
        let verifier = match self.nonces {
            Some(nonces) => verifier.with_server_nonces(nonces),
            None => verifier,
        };
        PopEdgeGuard::new(verifier, self.authority, self.policy)
    }
}

impl fmt::Debug for ProofSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofSettings")
            .field("policy", &self.policy)
            .field("resource", &self.resource)
            .field("authority", &self.authority)
            .field("nonces", &self.nonces)
            .field("freshness", &self.freshness)
            .field("leeway", &self.leeway)
            .finish_non_exhaustive()
    }
}

/// Every variant names the variable an operator has to fix.
#[derive(Debug, thiserror::Error)]
pub enum EdgeAuthConfigError {
    #[error("{ENV_AAUTH_MODE} must be one of enforce|shadow|off, got {0:?}")]
    InvalidMode(String),
    #[error("{ENV_AAUTH_DEV_UNENFORCED} must be one of true|1|yes when set, got {0:?}")]
    InvalidDevFlag(String),
    #[error(
        "{ENV_AAUTH_MODE}={0} leaves AAuth proof of possession unenforced, which is only for development and has to \
         be confirmed with {ENV_AAUTH_DEV_UNENFORCED}=true"
    )]
    UnenforcedWithoutDevFlag(EdgeAuthMode),
    #[error("{0} has to be set when the edge verifies AAuth proofs")]
    MissingRequired(&'static str),
    #[error("{ENV_AAUTH_RESOURCE_ISS}: {0}")]
    InvalidResource(#[source] EmptyResourceIdentifierError),
    #[error("{ENV_AAUTH_AUTHORITY}: {0}")]
    InvalidAuthority(#[source] EdgeAuthorityError),
    #[error("exactly one of {ENV_AAUTH_JWKS_PATH} or {ENV_AAUTH_JWKS_DISCOVERY}=true has to be set, got neither")]
    MissingJwksSource,
    #[error("exactly one of {ENV_AAUTH_JWKS_PATH} or {ENV_AAUTH_JWKS_DISCOVERY}=true has to be set, got both")]
    ConflictingJwksSource,
    #[error("{ENV_AAUTH_JWKS_DISCOVERY} must be one of true|false, got {0:?}")]
    InvalidJwksDiscovery(String),
    #[error("failed to read {var} at {path}: {source}")]
    ReadFile {
        var: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {var} at {path} as issuer key sets: {source}")]
    ParseJwks {
        var: &'static str,
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("{ENV_AAUTH_NONCE} must be one of required|optional, got {0:?}")]
    InvalidNonceRequirement(String),
    #[error("{ENV_AAUTH_CHALLENGE_KEY_PATH}: {0}")]
    InvalidChallengeKey(#[source] ChallengeKeyError),
    #[error("{ENV_AAUTH_CHALLENGE_KID}: {0}")]
    InvalidChallengeKid(#[source] EmptyChallengeKidError),
    #[error("{ENV_AAUTH_NONCE_TTL_SECS}: {0}")]
    InvalidNonceTtl(#[source] NonceTtlError),
    #[error("{var} must be a whole number of seconds, got {raw:?}")]
    InvalidSecs {
        var: &'static str,
        raw: String,
        #[source]
        source: ParseIntError,
    },
    #[error("{ENV_AAUTH_SIGNATURE_VALIDITY_SECS}: {0}")]
    InvalidSignatureValidity(#[source] SignatureValidityError),
    #[error("{var}: {source}")]
    InvalidClockTolerance {
        var: &'static str,
        #[source]
        source: ClockToleranceError,
    },
}

/// Whether [`ENV_AAUTH_DEV_UNENFORCED`] grants a mode below enforce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DevUnenforced {
    Unset,
    Granted,
}

impl DevUnenforced {
    fn from_env(env: &impl ReadEnv) -> Result<Self, EdgeAuthConfigError> {
        match optional_var(env, ENV_AAUTH_DEV_UNENFORCED) {
            None => Ok(Self::Unset),
            Some(raw)
                if DEV_UNENFORCED_VALUES
                    .iter()
                    .any(|value| raw.eq_ignore_ascii_case(value)) =>
            {
                Ok(Self::Granted)
            }
            Some(raw) => Err(EdgeAuthConfigError::InvalidDevFlag(raw)),
        }
    }
}

/// Whether issuer keys come from well-known discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JwksDiscovery {
    Enabled,
    Disabled,
}

impl JwksDiscovery {
    fn from_env(env: &impl ReadEnv) -> Result<Self, EdgeAuthConfigError> {
        match optional_var(env, ENV_AAUTH_JWKS_DISCOVERY) {
            None => Ok(Self::Disabled),
            Some(raw) if raw.eq_ignore_ascii_case("true") => Ok(Self::Enabled),
            Some(raw) if raw.eq_ignore_ascii_case("false") => Ok(Self::Disabled),
            Some(raw) => Err(EdgeAuthConfigError::InvalidJwksDiscovery(raw)),
        }
    }
}

fn mode_from_env(env: &impl ReadEnv) -> Result<EdgeAuthMode, EdgeAuthConfigError> {
    let Some(raw) = optional_var(env, ENV_AAUTH_MODE) else {
        return Ok(EdgeAuthMode::Enforce);
    };
    [EdgeAuthMode::Enforce, EdgeAuthMode::Shadow, EdgeAuthMode::Off]
        .into_iter()
        .find(|mode| raw.eq_ignore_ascii_case(mode.as_str()))
        .ok_or(EdgeAuthConfigError::InvalidMode(raw))
}

fn jwks_from_env(env: &impl ReadEnv, fs: &impl ReadFile) -> Result<Arc<dyn JwksResolver>, EdgeAuthConfigError> {
    match (optional_var(env, ENV_AAUTH_JWKS_PATH), JwksDiscovery::from_env(env)?) {
        (Some(path), JwksDiscovery::Disabled) => Ok(Arc::new(static_jwks(fs, &path)?)),
        (None, JwksDiscovery::Enabled) => Ok(Arc::new(discovered_jwks(env)?)),
        (None, JwksDiscovery::Disabled) => Err(EdgeAuthConfigError::MissingJwksSource),
        (Some(_), JwksDiscovery::Enabled) => Err(EdgeAuthConfigError::ConflictingJwksSource),
    }
}

fn static_jwks(fs: &impl ReadFile, path: &str) -> Result<StaticJwks, EdgeAuthConfigError> {
    let raw = read_file(fs, ENV_AAUTH_JWKS_PATH, path)?;
    let sets: HashMap<String, JwkSet> =
        serde_json::from_str(&raw).map_err(|source| EdgeAuthConfigError::ParseJwks {
            var: ENV_AAUTH_JWKS_PATH,
            path: path.to_owned(),
            source,
        })?;
    Ok(sets
        .into_iter()
        .fold(StaticJwks::new(), |jwks, (issuer, set)| jwks.with(issuer, set)))
}

fn discovered_jwks(
    env: &impl ReadEnv,
) -> Result<CachedJwksResolver<HttpJwksResolver, SystemTimeSource>, EdgeAuthConfigError> {
    let ttl_secs = optional_secs(env, ENV_AAUTH_JWKS_TTL_SECS)?.map_or(DEFAULT_TTL_SECS, i64::from);
    let issuers: Vec<String> = optional_var(env, ENV_AAUTH_JWKS_ALLOWED_ISSUERS)
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|issuer| !issuer.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let resolver = if issuers.is_empty() {
        HttpJwksResolver::new()
    } else {
        HttpJwksResolver::new().with_allowed_issuers(issuers)
    };
    Ok(CachedJwksResolver::new(resolver, SystemTimeSource).with_ttl_secs(ttl_secs))
}

fn nonces_from_env(env: &impl ReadEnv, fs: &impl ReadFile) -> Result<Option<ServerNonces>, EdgeAuthConfigError> {
    let requirement = nonce_requirement(env)?;
    let (key_path, kid) = match (
        requirement,
        optional_var(env, ENV_AAUTH_CHALLENGE_KEY_PATH),
        optional_var(env, ENV_AAUTH_CHALLENGE_KID),
    ) {
        (_, Some(key_path), Some(kid)) => (key_path, kid),
        (NonceRequirement::Optional, None, None) => return Ok(None),
        (_, None, _) => return Err(EdgeAuthConfigError::MissingRequired(ENV_AAUTH_CHALLENGE_KEY_PATH)),
        (_, Some(_), None) => return Err(EdgeAuthConfigError::MissingRequired(ENV_AAUTH_CHALLENGE_KID)),
    };
    let kid = ChallengeKid::new(kid).map_err(EdgeAuthConfigError::InvalidChallengeKid)?;
    let pem = read_file(fs, ENV_AAUTH_CHALLENGE_KEY_PATH, &key_path)?;
    let key = ChallengeKey::from_pkcs8_pem(&pem, kid).map_err(EdgeAuthConfigError::InvalidChallengeKey)?;
    let ttl = optional_secs(env, ENV_AAUTH_NONCE_TTL_SECS)?
        .map(NonceTtl::new)
        .transpose()
        .map_err(EdgeAuthConfigError::InvalidNonceTtl)?
        .unwrap_or_default();
    Ok(Some(ServerNonces::new(key, ttl, requirement)))
}

fn nonce_requirement(env: &impl ReadEnv) -> Result<NonceRequirement, EdgeAuthConfigError> {
    match optional_var(env, ENV_AAUTH_NONCE) {
        None => Ok(NonceRequirement::Optional),
        Some(raw) if raw.eq_ignore_ascii_case("required") => Ok(NonceRequirement::Required),
        Some(raw) if raw.eq_ignore_ascii_case("optional") => Ok(NonceRequirement::Optional),
        Some(raw) => Err(EdgeAuthConfigError::InvalidNonceRequirement(raw)),
    }
}

fn clock_tolerance(env: &impl ReadEnv, var: &'static str) -> Result<Option<ClockTolerance>, EdgeAuthConfigError> {
    optional_secs(env, var)?
        .map(ClockTolerance::new)
        .transpose()
        .map_err(|source| EdgeAuthConfigError::InvalidClockTolerance { var, source })
}

/// The trimmed value of `var`, or `None` when it is unset or blank, since a
/// value mounted from a file commonly carries a trailing newline.
fn optional_var(env: &impl ReadEnv, var: &str) -> Option<String> {
    env.var(var)
        .ok()
        .map(|raw| raw.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn required_var(env: &impl ReadEnv, var: &'static str) -> Result<String, EdgeAuthConfigError> {
    optional_var(env, var).ok_or(EdgeAuthConfigError::MissingRequired(var))
}

fn optional_secs(env: &impl ReadEnv, var: &'static str) -> Result<Option<u32>, EdgeAuthConfigError> {
    optional_var(env, var)
        .map(|raw| {
            raw.parse::<u32>()
                .map_err(|source| EdgeAuthConfigError::InvalidSecs { var, raw, source })
        })
        .transpose()
}

fn read_file(fs: &impl ReadFile, var: &'static str, path: &str) -> Result<String, EdgeAuthConfigError> {
    fs.read_to_string(Path::new(path))
        .map_err(|source| EdgeAuthConfigError::ReadFile {
            var,
            path: path.to_owned(),
            source,
        })
}

#[cfg(test)]
mod tests;
