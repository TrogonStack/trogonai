//! Verified workload JWT ingress (JWKS / HS256 / static RSA PEM), aligned with Trogon identity slices.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{AlgorithmParameters, EllipticCurve, JwkSet},
};
use tokio::sync::Mutex;
use tracing::warn;

use crate::authz::{GatewayIdentity, IdentitySource};
use crate::rpc_codes;

pub use crate::agent_identity::{ActChainEntry, AgentIdentityMode, MAX_ACT_CHAIN_DEPTH};

/// Optional agent-identity claims parsed from a verified JWT payload.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct VerifiedJwtClaims {
    pub agent_id: Option<String>,
    pub agent_version: Option<String>,
    pub wkl: Option<String>,
    pub wkl_attested_at: Option<i64>,
    pub auth_method: Option<String>,
    pub act_chain: Option<Vec<ActChainEntry>>,
    pub purpose: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct JwtResolution {
    pub identity: GatewayIdentity,
    pub claims: VerifiedJwtClaims,
}

const JWKS_CACHE_TTL: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JwtMode {
    Off,
    Validate,
    Require,
}

#[derive(Clone, Debug)]
pub struct JwtIngressConfig {
    pub mode: JwtMode,
    pub agent_identity_mode: AgentIdentityMode,
    pub issuers: HashSet<String>,
    /// Issuers allowed to mint forgeable identity claims (`agent_id`, `wkl`, `act_chain`, …).
    pub trusted_mint_issuers: HashSet<String>,
    pub audience: String,
    pub leeway_secs: u64,
    pub tenant_claim_key: String,
    pub bearer_header_name: String,
    pub hs256_secret: Option<Vec<u8>>,
    pub rsa_public_key_pem: Option<String>,
    pub jwks_uri: Option<String>,
}

impl JwtIngressConfig {
    pub fn from_env<E: trogon_std::env::ReadEnv>(env: &E) -> Result<Self, String> {
        use std::env::VarError;

        const ENV_MODE: &str = "MCP_GATEWAY_JWT_MODE";
        const ENV_ISSUERS: &str = "MCP_GATEWAY_JWT_ISSUERS";
        const ENV_AUDIENCE: &str = "MCP_GATEWAY_JWT_AUDIENCE";
        const ENV_LEEWAY: &str = "MCP_GATEWAY_JWT_LEEWAY_SECS";
        const ENV_TENANT_CLAIM: &str = "MCP_GATEWAY_JWT_TENANT_CLAIM";
        const ENV_BEARER_HEADER: &str = "MCP_GATEWAY_JWT_BEARER_HEADER";
        const ENV_HS256: &str = "MCP_GATEWAY_JWT_HS256_SECRET";
        const ENV_RSA_PEM: &str = "MCP_GATEWAY_JWT_RSA_PUBLIC_KEY_PEM";
        const ENV_JWKS_URI: &str = "MCP_GATEWAY_JWT_JWKS_URI";
        const ENV_AGENT_IDENTITY: &str = "MCP_GATEWAY_AGENT_IDENTITY";
        const ENV_TRUSTED_MINT_ISSUERS: &str = "MCP_GATEWAY_TRUSTED_MINT_ISSUERS";
        const ENV_GATEWAY_AUDIENCE: &str = "MCP_GATEWAY_AUDIENCE";

        let raw_mode = env.var(ENV_MODE).unwrap_or_else(|_| "off".to_string());
        let mode = match raw_mode.trim().to_ascii_lowercase().as_str() {
            "" | "off" => JwtMode::Off,
            "validate" => JwtMode::Validate,
            "require" => JwtMode::Require,
            other => return Err(format!("{ENV_MODE} must be off|validate|require, got {other}")),
        };

        let raw_agent_identity = env.var(ENV_AGENT_IDENTITY).unwrap_or_else(|_| "off".to_string());
        let agent_identity_mode = match raw_agent_identity.trim().to_ascii_lowercase().as_str() {
            "" | "off" => AgentIdentityMode::Off,
            "shadow" => AgentIdentityMode::Shadow,
            "enforce" => AgentIdentityMode::Enforce,
            other => {
                return Err(format!("{ENV_AGENT_IDENTITY} must be off|shadow|enforce, got {other}"));
            }
        };

        let issuers: HashSet<String> = if mode == JwtMode::Off {
            HashSet::new()
        } else {
            let raw = env.var(ENV_ISSUERS).map_err(|e| match e {
                VarError::NotPresent => format!("{ENV_ISSUERS} is required when {ENV_MODE} is not off"),
                e => format!("reading {ENV_ISSUERS}: {e}"),
            })?;
            let set: HashSet<String> = raw
                .split(',')
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .map(|p| p.to_string())
                .collect();
            if set.is_empty() {
                return Err(format!(
                    "{ENV_ISSUERS} must list at least one issuer when {ENV_MODE} is not off"
                ));
            }
            set
        };

        let audience = env
            .var(ENV_GATEWAY_AUDIENCE)
            .or_else(|_| env.var(ENV_AUDIENCE))
            .unwrap_or_else(|_| "trogon-mcp-gateway".to_string());

        let trusted_mint_issuers: HashSet<String> = env
            .var(ENV_TRUSTED_MINT_ISSUERS)
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(|p| p.trim())
                    .filter(|p| !p.is_empty())
                    .map(|p| p.to_string())
                    .collect::<HashSet<String>>()
            })
            .filter(|set: &HashSet<String>| !set.is_empty())
            .unwrap_or_else(|| issuers.clone());
        let leeway_secs = env
            .var(ENV_LEEWAY)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(60);

        let tenant_claim_key = env
            .var(ENV_TENANT_CLAIM)
            .unwrap_or_else(|_| "https://trogon.ai/tenant".to_string());

        let bearer_header_name = env
            .var(ENV_BEARER_HEADER)
            .unwrap_or_else(|_| "authorization".to_string());

        let hs256_secret = env
            .var(ENV_HS256)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.into_bytes());

        let rsa_public_key_pem = env.var(ENV_RSA_PEM).ok().filter(|s| !s.trim().is_empty());

        let jwks_uri = env.var(ENV_JWKS_URI).ok().filter(|s| !s.trim().is_empty());

        let cfg = Self {
            mode,
            agent_identity_mode,
            issuers,
            trusted_mint_issuers,
            audience,
            leeway_secs,
            tenant_claim_key,
            bearer_header_name,
            hs256_secret,
            rsa_public_key_pem,
            jwks_uri,
        };

        cfg.validate_material()?;
        Ok(cfg)
    }

    fn validate_material(&self) -> Result<(), String> {
        if self.mode == JwtMode::Off {
            return Ok(());
        }
        let has_rsa = self.rsa_public_key_pem.as_ref().is_some_and(|p| !p.trim().is_empty());
        let has_hs = self.hs256_secret.as_ref().is_some_and(|s| !s.is_empty());
        let has_jwks = self.jwks_uri.as_ref().is_some_and(|u| !u.trim().is_empty());
        if !has_rsa && !has_hs && !has_jwks {
            return Err(
                "when MCP_GATEWAY_JWT_MODE is not off, configure one of MCP_GATEWAY_JWT_HS256_SECRET, MCP_GATEWAY_JWT_RSA_PUBLIC_KEY_PEM, or MCP_GATEWAY_JWT_JWKS_URI".into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct JwkCacheEntry {
    keys: JwkSet,
    fetched_at: Instant,
}

pub struct JwtValidator {
    cfg: JwtIngressConfig,
    static_rsa_der: Option<Vec<u8>>,
    jwks_cache: Mutex<Option<JwkCacheEntry>>,
    http: reqwest::Client,
}

#[derive(Debug)]
pub struct IdentityDeny {
    pub code: i32,
    pub message: String,
}

impl JwtValidator {
    pub fn try_new(cfg: JwtIngressConfig) -> Result<Arc<Self>, String> {
        let static_rsa_der = if let Some(pem_raw) = &cfg.rsa_public_key_pem {
            let trimmed = pem_raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                let pem = pem::parse(trimmed.trim())
                    .map_err(|e| format!("MCP_GATEWAY_JWT_RSA_PUBLIC_KEY_PEM: invalid PEM ({e})"))?;
                Some(pem.contents().to_vec())
            }
        } else {
            None
        };

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("JWKS HTTP client: {e}"))?;

        Ok(Arc::new(Self {
            cfg,
            static_rsa_der,
            jwks_cache: Mutex::new(None),
            http,
        }))
    }

    pub fn disabled() -> Result<Arc<Self>, String> {
        Self::try_new(JwtIngressConfig {
            mode: JwtMode::Off,
            agent_identity_mode: AgentIdentityMode::Off,
            issuers: HashSet::new(),
            trusted_mint_issuers: HashSet::new(),
            audience: String::new(),
            leeway_secs: 60,
            tenant_claim_key: "tenant".into(),
            bearer_header_name: "authorization".into(),
            hs256_secret: None,
            rsa_public_key_pem: None,
            jwks_uri: None,
        })
    }

    #[must_use]
    pub fn mode(&self) -> JwtMode {
        self.cfg.mode
    }

    #[must_use]
    pub fn agent_identity_mode(&self) -> AgentIdentityMode {
        self.cfg.agent_identity_mode
    }

    /// When true, forgeable tenant headers must not reach backends.
    #[must_use]
    pub fn jwt_controls_transport(&self) -> bool {
        !matches!(self.cfg.mode, JwtMode::Off)
    }

    /// Require a valid Bearer token for gated MCP methods (SpiceDB-eligible RPC).
    #[must_use]
    pub fn jwt_required_for_gate(&self, requires_spicedb: bool) -> bool {
        requires_spicedb && matches!(self.cfg.mode, JwtMode::Require)
    }

    pub async fn resolve(
        &self,
        bearer_token: Option<&str>,
        legacy_tenant_header: Option<&str>,
        jwt_strict: bool,
    ) -> Result<GatewayIdentity, IdentityDeny> {
        Ok(self
            .resolve_with_claims(bearer_token, legacy_tenant_header, jwt_strict, None)
            .await?
            .identity)
    }

    pub async fn resolve_with_claims(
        &self,
        bearer_token: Option<&str>,
        legacy_tenant_header: Option<&str>,
        jwt_strict: bool,
        jsonrpc_method: Option<&str>,
    ) -> Result<JwtResolution, IdentityDeny> {
        let mut resolution = match self.cfg.mode {
            JwtMode::Off => JwtResolution {
                identity: legacy_identity(legacy_tenant_header),
                claims: VerifiedJwtClaims::default(),
            },
            JwtMode::Validate | JwtMode::Require => {
                self.resolve_validate_or_require_with_claims(bearer_token, legacy_tenant_header, jwt_strict)
                    .await?
            }
        };

        if self.cfg.agent_identity_mode != AgentIdentityMode::Off
            && let Some(deny) = apply_agent_identity_ingress(
                self.cfg.agent_identity_mode,
                &self.cfg.trusted_mint_issuers,
                &self.cfg.audience,
                resolution.identity.issuer.as_deref(),
                jsonrpc_method,
                &mut resolution.claims,
            )
        {
            return Err(deny);
        }

        Ok(resolution)
    }

    async fn resolve_validate_or_require_with_claims(
        &self,
        bearer_token: Option<&str>,
        legacy_tenant_header: Option<&str>,
        jwt_strict: bool,
    ) -> Result<JwtResolution, IdentityDeny> {
        let Some(token) = bearer_token.filter(|s| !s.is_empty()) else {
            if jwt_strict {
                return Err(IdentityDeny {
                    code: rpc_codes::AUTH_REQUIRED,
                    message: "authentication required".into(),
                });
            }
            return Ok(JwtResolution {
                identity: legacy_identity(legacy_tenant_header),
                claims: VerifiedJwtClaims::default(),
            });
        };

        match self.validate_and_extract(token).await {
            Ok(resolution) => Ok(resolution),
            Err(e) if e.expired_token => Err(IdentityDeny {
                code: rpc_codes::AUTH_EXPIRED,
                message: e.message,
            }),
            Err(e)
                if e.audience_mismatch
                    && (jwt_strict || self.cfg.agent_identity_mode == AgentIdentityMode::Enforce) =>
            {
                Err(IdentityDeny {
                    code: rpc_codes::AUDIENCE_MISMATCH,
                    message: "audience_mismatch".into(),
                })
            }
            Err(e) if jwt_strict => Err(IdentityDeny {
                code: rpc_codes::INVALID_TOKEN,
                message: e.message,
            }),
            Err(e) => {
                warn!(error = %e.message, "JWT validation failed; falling back to legacy tenant header");
                Ok(JwtResolution {
                    identity: legacy_identity(legacy_tenant_header),
                    claims: VerifiedJwtClaims::default(),
                })
            }
        }
    }

    #[allow(clippy::too_many_lines)] // JWKS/RSA branching is intentionally linear.
    async fn validate_and_extract(&self, token: &str) -> Result<JwtResolution, ValidateErr> {
        let header = decode_header(token).map_err(|e| ValidateErr::new(false, format!("invalid jwt header ({e})")))?;

        let alg = header.alg;
        let mut validation = Validation::new(alg);
        validation.leeway = self.cfg.leeway_secs;

        let iss_vec: Vec<String> = self.cfg.issuers.iter().cloned().collect();
        let iss_refs: Vec<&str> = iss_vec.iter().map(String::as_str).collect();
        validation.set_issuer(&iss_refs);
        validation.validate_aud = true;
        validation.set_audience(&[self.cfg.audience.as_str()]);

        let key = decoding_key_for_token(self, alg, header.kid.as_deref()).await?;
        let data = decode::<serde_json::Value>(token, &key, &validation).map_err(classify_decode_error)?;

        let claims = data.claims;
        let sub = claims
            .get("sub")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ValidateErr::new(false, "missing sub claim".into()))?;

        let iss = claims
            .get("iss")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);
        let jti = claims
            .get("jti")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);

        let tenant_claim = tenant_from_claim_json(&claims, &self.cfg.tenant_claim_key);
        let mut agent_claims = parse_verified_claims(&claims);
        if !aud_claim_matches_gateway(&claims, &self.cfg.audience) {
            return Err(ValidateErr::audience_mismatch());
        }
        strip_untrusted_minted_identity_claims(iss.as_deref(), &self.cfg.trusted_mint_issuers, &mut agent_claims);
        Ok(JwtResolution {
            identity: GatewayIdentity {
                tenant: tenant_claim.filter(|t| !t.is_empty()),
                caller_sub: Some(sub.to_string()),
                issuer: iss,
                jti,
                source: IdentitySource::Jwt,
            },
            claims: agent_claims,
        })
    }

    #[must_use]
    pub fn bearer_header_name_normalized(&self) -> String {
        self.cfg.bearer_header_name.trim().to_ascii_lowercase()
    }

    async fn load_jwk_set_cached(&self, uri: &str) -> Result<JwkSet, ValidateErr> {
        let mut cache = self.jwks_cache.lock().await;
        let now_ok = cache
            .as_ref()
            .map(|c| c.fetched_at.elapsed() < JWKS_CACHE_TTL && !c.keys.keys.is_empty())
            .unwrap_or(false);
        if !now_ok {
            let body = self
                .http
                .get(uri)
                .send()
                .await
                .map_err(|e| ValidateErr::new(false, format!("jwks fetch ({e})")))?
                .error_for_status()
                .map_err(|e| ValidateErr::new(false, format!("jwks HTTP ({e})")))?
                .text()
                .await
                .map_err(|e| ValidateErr::new(false, format!("jwks body ({e})")))?;
            let doc: JwkSet =
                serde_json::from_str(&body).map_err(|e| ValidateErr::new(false, format!("jwks json ({e})")))?;
            if doc.keys.is_empty() {
                return Err(ValidateErr::new(false, "JWKS keys array is empty".into()));
            }
            *cache = Some(JwkCacheEntry {
                keys: doc.clone(),
                fetched_at: Instant::now(),
            });
            Ok(doc)
        } else {
            Ok(cache.as_ref().expect("jwks cache").keys.clone())
        }
    }
}

#[derive(Debug)]
struct ValidateErr {
    expired_token: bool,
    audience_mismatch: bool,
    message: String,
}

impl ValidateErr {
    fn new(expired_token: bool, message: String) -> Self {
        Self {
            expired_token,
            audience_mismatch: false,
            message,
        }
    }

    fn audience_mismatch() -> Self {
        Self {
            expired_token: false,
            audience_mismatch: true,
            message: "audience_mismatch".into(),
        }
    }
}

fn classify_decode_error(e: jsonwebtoken::errors::Error) -> ValidateErr {
    use jsonwebtoken::errors::ErrorKind;
    let expired_token = matches!(e.kind(), ErrorKind::ExpiredSignature);
    let audience_mismatch = matches!(e.kind(), ErrorKind::InvalidAudience);
    ValidateErr {
        expired_token,
        audience_mismatch,
        message: e.to_string(),
    }
}

pub fn parse_verified_claims(claims: &serde_json::Value) -> VerifiedJwtClaims {
    serde_json::from_value(claims.clone()).unwrap_or_default()
}

/// Strip forgeable identity claims when the bearer was not minted by a trusted issuer.
pub fn strip_untrusted_minted_identity_claims(
    issuer: Option<&str>,
    trusted_mint_issuers: &HashSet<String>,
    claims: &mut VerifiedJwtClaims,
) {
    let trusted = issuer.is_some_and(|iss| trusted_mint_issuers.contains(iss));
    if trusted {
        return;
    }
    claims.agent_id = None;
    claims.wkl = None;
    claims.wkl_attested_at = None;
    claims.auth_method = None;
    claims.act_chain = None;
}

fn apply_agent_identity_ingress(
    mode: AgentIdentityMode,
    _trusted_mint_issuers: &HashSet<String>,
    _gateway_audience: &str,
    _issuer: Option<&str>,
    jsonrpc_method: Option<&str>,
    claims: &mut VerifiedJwtClaims,
) -> Option<IdentityDeny> {
    if let Some(deny) = enforce_act_chain_violations(mode, claims.act_chain.as_deref()) {
        return Some(deny);
    }

    if mode == AgentIdentityMode::Enforce {
        return enforce_agent_identity_claims(claims);
    }

    if mode == AgentIdentityMode::Shadow && jsonrpc_method == Some("tools/call") {
        log_shadow_agent_identity_violations(claims);
    }

    None
}

pub fn enforce_act_chain_violations(mode: AgentIdentityMode, chain: Option<&[ActChainEntry]>) -> Option<IdentityDeny> {
    if mode != AgentIdentityMode::Enforce {
        return None;
    }
    let entries = chain?;
    if entries.len() > MAX_ACT_CHAIN_DEPTH {
        return Some(IdentityDeny {
            code: rpc_codes::ACT_CHAIN_DEPTH_EXCEEDED,
            message: "act_chain_depth_exceeded".into(),
        });
    }
    if act_chain_has_agent_wkl_loop(entries) {
        return Some(IdentityDeny {
            code: rpc_codes::ACT_CHAIN_LOOP_DETECTED,
            message: "act_chain_loop_detected".into(),
        });
    }
    None
}

pub fn enforce_header_act_chain_violations(mode: AgentIdentityMode, ingress_raw: Option<&str>) -> Option<IdentityDeny> {
    if mode != AgentIdentityMode::Enforce {
        return None;
    }
    let raw = ingress_raw.filter(|s| !s.trim().is_empty())?;
    let entries = crate::act_chain::parse_act_chain(raw).ok()?;
    enforce_act_chain_violations(mode, Some(entries.as_slice()))
}

fn enforce_agent_identity_claims(claims: &VerifiedJwtClaims) -> Option<IdentityDeny> {
    if requires_wkl_for_auth_method(claims.auth_method.as_deref()) && claims.wkl.as_ref().is_none_or(|w| w.is_empty()) {
        return Some(IdentityDeny {
            code: rpc_codes::AGENT_IDENTITY_REQUIRED,
            message: "agent_identity_required".into(),
        });
    }
    if is_spiffe_wkl(claims.wkl.as_deref()) && claims.agent_id.as_ref().is_none_or(|a| a.is_empty()) {
        return Some(IdentityDeny {
            code: rpc_codes::AGENT_IDENTITY_REQUIRED,
            message: "agent_identity_required".into(),
        });
    }
    None
}

fn log_shadow_agent_identity_violations(claims: &VerifiedJwtClaims) {
    let missing_agent_id = claims.agent_id.as_ref().is_none_or(|value| value.is_empty());
    let missing_wkl = claims.wkl.as_ref().is_none_or(|value| value.is_empty());
    if missing_agent_id || missing_wkl {
        warn!(
            event = "agent_identity_violation",
            missing_agent_id, missing_wkl, "agent identity claims missing on tools/call"
        );
    }
}

/// Shadow-mode logging hook for `tools/call` missing-claim checks (unit tests).
pub fn check_agent_identity_violations(mode: AgentIdentityMode, jsonrpc_method: &str, claims: &VerifiedJwtClaims) {
    if mode == AgentIdentityMode::Shadow && jsonrpc_method == "tools/call" {
        log_shadow_agent_identity_violations(claims);
    }
}

fn requires_wkl_for_auth_method(auth_method: Option<&str>) -> bool {
    matches!(
        auth_method.map(str::to_ascii_lowercase).as_deref(),
        Some("svid") | Some("spire")
    )
}

fn is_spiffe_wkl(wkl: Option<&str>) -> bool {
    wkl.is_some_and(|w| w.starts_with("spiffe://"))
}

fn aud_claim_matches_gateway(claims: &serde_json::Value, expected_aud: &str) -> bool {
    match claims.get("aud") {
        Some(serde_json::Value::String(aud)) => aud == expected_aud,
        Some(serde_json::Value::Array(values)) => {
            values.iter().filter_map(|v| v.as_str()).any(|aud| aud == expected_aud)
        }
        _ => false,
    }
}

fn act_chain_has_agent_wkl_loop(entries: &[ActChainEntry]) -> bool {
    let mut seen = HashSet::new();
    for entry in entries {
        let Some(agent_id) = entry.agent_id.as_deref().filter(|a| !a.is_empty()) else {
            continue;
        };
        let Some(wkl) = entry.wkl.as_deref().filter(|w| !w.is_empty()) else {
            continue;
        };
        let key = (agent_id.to_string(), wkl.to_string());
        if !seen.insert(key) {
            return true;
        }
    }
    false
}

fn legacy_identity(legacy_tenant_header: Option<&str>) -> GatewayIdentity {
    let tenant = legacy_tenant_header.map(std::string::ToString::to_string);
    let source = if tenant.is_some() {
        IdentitySource::LegacyHeader
    } else {
        IdentitySource::Anonymous
    };
    GatewayIdentity {
        tenant,
        caller_sub: None,
        issuer: None,
        jti: None,
        source,
    }
}

fn tenant_from_claim_json(claims: &serde_json::Value, tenant_claim_cfg: &str) -> Option<String> {
    if tenant_claim_cfg.trim().eq_ignore_ascii_case("tenant") {
        return claims
            .get("tenant")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);
    }
    claims
        .get(tenant_claim_cfg)
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
        .or_else(|| {
            claims
                .get("tenant")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string)
        })
}

async fn decoding_key_for_token(
    validator: &JwtValidator,
    alg: Algorithm,
    kid: Option<&str>,
) -> Result<DecodingKey, ValidateErr> {
    match alg {
        Algorithm::HS256 => {
            let Some(secret) = validator.cfg.hs256_secret.as_ref() else {
                return Err(ValidateErr::new(
                    false,
                    "token uses HS256 but MCP_GATEWAY_JWT_HS256_SECRET is not set".into(),
                ));
            };
            Ok(DecodingKey::from_secret(secret))
        }
        Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512 => {
            if let Some(der) = &validator.static_rsa_der {
                Ok(DecodingKey::from_rsa_der(der))
            } else if let Some(uri) = validator.cfg.jwks_uri.clone() {
                jwks_decoding_key(validator, alg, uri, kid).await
            } else {
                Err(ValidateErr::new(
                    false,
                    "token uses RSA but neither MCP_GATEWAY_JWT_RSA_PUBLIC_KEY_PEM nor MCP_GATEWAY_JWT_JWKS_URI is configured"
                        .into(),
                ))
            }
        }
        Algorithm::ES256 | Algorithm::ES384 => {
            let Some(uri) = validator.cfg.jwks_uri.clone() else {
                return Err(ValidateErr::new(
                    false,
                    "token uses EC signature but MCP_GATEWAY_JWT_JWKS_URI is not configured".into(),
                ));
            };
            jwks_decoding_key(validator, alg, uri, kid).await
        }
        _ => Err(ValidateErr::new(false, format!("unsupported jwt algorithm {:?}", alg))),
    }
}

async fn jwks_decoding_key(
    validator: &JwtValidator,
    alg: Algorithm,
    uri: String,
    kid_hint: Option<&str>,
) -> Result<DecodingKey, ValidateErr> {
    let jwks = validator.load_jwk_set_cached(&uri).await?;
    let jwk = pick_jwk(&jwks, alg, kid_hint)?;
    DecodingKey::from_jwk(jwk).map_err(|e| ValidateErr::new(false, format!("jwks decoding key ({e})")))
}

fn jwk_compatible_with_alg(jwk: &jsonwebtoken::jwk::Jwk, alg: Algorithm) -> bool {
    match (&jwk.algorithm, alg) {
        (AlgorithmParameters::RSA(_), Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512) => true,
        (AlgorithmParameters::EllipticCurve(ec), Algorithm::ES256) => ec.curve == EllipticCurve::P256,
        (AlgorithmParameters::EllipticCurve(ec), Algorithm::ES384) => ec.curve == EllipticCurve::P384,
        _ => false,
    }
}

fn pick_jwk<'a>(
    jwks: &'a JwkSet,
    alg: Algorithm,
    kid_hint: Option<&str>,
) -> Result<&'a jsonwebtoken::jwk::Jwk, ValidateErr> {
    let compat: Vec<&jsonwebtoken::jwk::Jwk> = jwks.keys.iter().filter(|k| jwk_compatible_with_alg(k, alg)).collect();
    if compat.is_empty() {
        return Err(ValidateErr::new(
            false,
            "JWKS contained no keys compatible with the token algorithm".into(),
        ));
    }
    if let Some(kid) = kid_hint
        && let Some(jwk) = compat.iter().copied().find(|j| j.common.key_id.as_deref() == Some(kid))
    {
        return Ok(jwk);
    }
    if compat.len() == 1 {
        return Ok(compat[0]);
    }
    Err(ValidateErr::new(
        false,
        "jwt kid did not match any JWKS keys (or JWKS requires kid)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use std::collections::HashSet;
    use std::sync::LazyLock;
    use std::time::{SystemTime, UNIX_EPOCH};

    static HS_ISSUER: &str = "https://issuer.test/";
    static HS_AUD: &str = "trogon-mcp-gateway";
    static HS_SECRET: LazyLock<Vec<u8>> = LazyLock::new(|| b"super-secret-demo-key-change-me".to_vec());

    #[allow(clippy::cast_possible_truncation)] // JWT exp fits i64 until year 292277026596.
    fn exp_unix_secs_ahead(seconds: i64) -> serde_json::Value {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        serde_json::Value::Number((now + seconds).into())
    }

    fn cfg_hs256() -> JwtIngressConfig {
        let mut iss = HashSet::new();
        iss.insert(HS_ISSUER.into());
        JwtIngressConfig {
            mode: JwtMode::Validate,
            agent_identity_mode: AgentIdentityMode::Off,
            issuers: iss.clone(),
            trusted_mint_issuers: iss,
            audience: HS_AUD.into(),
            leeway_secs: 60,
            tenant_claim_key: "tenant".into(),
            bearer_header_name: "authorization".into(),
            hs256_secret: Some(HS_SECRET.clone()),
            rsa_public_key_pem: None,
            jwks_uri: None,
        }
    }

    #[tokio::test]
    async fn hs256_resolve_ok() {
        let validator = JwtValidator::try_new(cfg_hs256()).expect("jwt cfg");
        let claims = serde_json::json!({
            "sub": "principal-123",
            "iss": HS_ISSUER,
            "aud": HS_AUD,
            "tenant": "acme",
            "exp": exp_unix_secs_ahead(3_600),
        });

        let enc = EncodingKey::from_secret(&HS_SECRET);
        let mut hdr = Header::new(Algorithm::HS256);
        hdr.typ = Some("JWT".into());
        let token = encode(&hdr, &claims, &enc).expect("jwt encode");

        let id = validator
            .resolve(Some(token.as_str()), Some("evil-tenant-header"), false)
            .await
            .expect("resolve ok");
        assert_eq!(id.caller_sub.as_deref(), Some("principal-123"));
        assert_eq!(id.tenant.as_deref(), Some("acme"));
        assert_eq!(id.source, IdentitySource::Jwt);
    }

    #[tokio::test]
    async fn hs256_legacy_fallback_when_validate_missing_bearer() {
        let validator = JwtValidator::try_new(cfg_hs256()).expect("jwt cfg");
        let id = validator.resolve(None, Some("legacy-org"), false).await.expect("ok");
        assert_eq!(id.source, IdentitySource::LegacyHeader);
        assert_eq!(id.tenant.as_deref(), Some("legacy-org"));
    }

    #[tokio::test]
    async fn require_mode_denies_without_bearer_when_strict() {
        let mut cfg = cfg_hs256();
        cfg.mode = JwtMode::Require;
        let validator = JwtValidator::try_new(cfg).expect("jwt cfg");
        let err = validator.resolve(None, Some("x"), true).await.expect_err("deny");
        assert_eq!(err.code, rpc_codes::AUTH_REQUIRED);
    }

    #[test]
    fn tenant_claim_namespaced_fallback() {
        let claims = serde_json::json!({
            "tenant": "from-plain",
        });
        let t = tenant_from_claim_json(&claims, "https://trogon.ai/tenant");
        assert_eq!(t.as_deref(), Some("from-plain"));
    }
}
