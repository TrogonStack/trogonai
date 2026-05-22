//! Verified workload JWT ingress (JWKS / HS256 / static RSA PEM), aligned with Trogon identity slices.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use rsa::BigUint;
use rsa::RsaPublicKey;
use rsa::pkcs8::EncodePublicKey;
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::warn;

use crate::authz::{GatewayIdentity, IdentitySource};
use crate::rpc_codes;

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
    pub issuers: HashSet<String>,
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

        let raw_mode = env
            .var(ENV_MODE)
            .unwrap_or_else(|_| "off".to_string());
        let mode = match raw_mode.trim().to_ascii_lowercase().as_str() {
            "" | "off" => JwtMode::Off,
            "validate" => JwtMode::Validate,
            "require" => JwtMode::Require,
            other => return Err(format!("{ENV_MODE} must be off|validate|require, got {other}")),
        };

        let issuers: HashSet<String> = if mode == JwtMode::Off {
            HashSet::new()
        } else {
            let raw = env
                .var(ENV_ISSUERS)
                .map_err(|e| match e {
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
            .var(ENV_AUDIENCE)
            .unwrap_or_else(|_| "trogon-mcp-gateway".to_string());
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
            issuers,
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
    keys: Vec<(Option<String>, Vec<u8>)>,
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
                let pem = pem::parse(trimmed.trim()).map_err(|e| format!("MCP_GATEWAY_JWT_RSA_PUBLIC_KEY_PEM: invalid PEM ({e})"))?;
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
            issuers: HashSet::new(),
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
        match self.cfg.mode {
            JwtMode::Off => Ok(legacy_identity(legacy_tenant_header)),
            JwtMode::Validate | JwtMode::Require => self.resolve_validate_or_require(bearer_token, legacy_tenant_header, jwt_strict).await,
        }
    }

    async fn resolve_validate_or_require(
        &self,
        bearer_token: Option<&str>,
        legacy_tenant_header: Option<&str>,
        jwt_strict: bool,
    ) -> Result<GatewayIdentity, IdentityDeny> {
        let Some(token) = bearer_token.filter(|s| !s.is_empty()) else {
            if jwt_strict {
                return Err(IdentityDeny {
                    code: rpc_codes::AUTH_REQUIRED,
                    message: "authentication required".into(),
                });
            }
            return Ok(legacy_identity(legacy_tenant_header));
        };

        match self.validate_and_extract(token).await {
            Ok(ident) => Ok(ident),
            Err(e) if e.expired_token => Err(IdentityDeny {
                code: rpc_codes::AUTH_EXPIRED,
                message: e.message,
            }),
            Err(e) if jwt_strict => Err(IdentityDeny {
                code: rpc_codes::INVALID_TOKEN,
                message: e.message,
            }),
            Err(e) => {
                warn!(error = %e.message, "JWT validation failed; falling back to legacy tenant header");
                Ok(legacy_identity(legacy_tenant_header))
            }
        }
    }

    #[allow(clippy::too_many_lines)] // JWKS/RSA branching is intentionally linear.
    async fn validate_and_extract(&self, token: &str) -> Result<GatewayIdentity, ValidateErr> {
        let header =
            decode_header(token).map_err(|e| ValidateErr::new(false, format!("invalid jwt header ({e})")))?;

        let alg = header.alg;
        let mut validation = Validation::new(alg);
        validation.leeway = self.cfg.leeway_secs;

        let iss_vec: Vec<String> = self.cfg.issuers.iter().cloned().collect();
        let iss_refs: Vec<&str> = iss_vec.iter().map(String::as_str).collect();
        validation.set_issuer(&iss_refs);
        validation.validate_aud = true;
        validation.set_audience(&[self.cfg.audience.as_str()]);

        let key =
            decoding_key_for_token(self, alg, header.kid.as_deref()).await?;
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
        Ok(GatewayIdentity {
            tenant: tenant_claim.filter(|t| !t.is_empty()),
            caller_sub: Some(sub.to_string()),
            issuer: iss,
            jti,
            source: IdentitySource::Jwt,
        })
    }

    #[must_use]
    pub fn bearer_header_name_normalized(&self) -> String {
        self.cfg.bearer_header_name.trim().to_ascii_lowercase()
    }

    async fn load_jwk_entries(&self, uri: &str) -> Result<Vec<(Option<String>, Vec<u8>)>, ValidateErr> {
        let mut cache = self.jwks_cache.lock().await;
        let now_ok = cache
            .as_ref()
            .map(|c| c.fetched_at.elapsed() < JWKS_CACHE_TTL && !c.keys.is_empty())
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
            let doc: JwkDoc =
                serde_json::from_str(&body).map_err(|e| ValidateErr::new(false, format!("jwks json ({e})")))?;
            let mut out = Vec::new();
            for jwk in doc.keys {
                if jwk.kty != "RSA" {
                    continue;
                }
                let Some(ref n_b64) = jwk.n else {
                    continue;
                };
                let Some(ref e_b64) = jwk.e else {
                    continue;
                };
                let der = jwk_rsa_to_spki_der(n_b64, e_b64)
                    .map_err(|msg| ValidateErr::new(false, msg))?;
                out.push((jwk.kid, der));
            }
            if out.is_empty() {
                return Err(ValidateErr::new(false, "JWKS contained no RSA keys".into()));
            }
            *cache = Some(JwkCacheEntry {
                keys: out.clone(),
                fetched_at: Instant::now(),
            });
            Ok(out)
        } else {
            Ok(cache.as_ref().expect("jwks cache").keys.clone())
        }
    }
}

#[derive(Debug)]
struct ValidateErr {
    expired_token: bool,
    message: String,
}

impl ValidateErr {
    fn new(expired_token: bool, message: String) -> Self {
        Self {
            expired_token,
            message,
        }
    }
}

fn classify_decode_error(e: jsonwebtoken::errors::Error) -> ValidateErr {
    use jsonwebtoken::errors::ErrorKind;
    let expired_token = matches!(e.kind(), ErrorKind::ExpiredSignature);
    ValidateErr::new(expired_token, e.to_string())
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
        return claims.get("tenant").and_then(|v| v.as_str()).map(std::string::ToString::to_string);
    }
    claims
        .get(tenant_claim_cfg)
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
        .or_else(|| claims.get("tenant").and_then(|v| v.as_str()).map(std::string::ToString::to_string))
}

async fn decoding_key_for_token(
    validator: &JwtValidator,
    alg: Algorithm,
    kid: Option<&str>,
) -> Result<DecodingKey, ValidateErr> {
    match alg {
        Algorithm::HS256 => {
            let Some(secret) = validator.cfg.hs256_secret.as_ref() else {
                return Err(ValidateErr::new(false, "token uses HS256 but MCP_GATEWAY_JWT_HS256_SECRET is not set".into()));
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
        _ => Err(ValidateErr::new(
            false,
            format!("unsupported jwt algorithm {:?}", alg),
        )),
    }
}

#[derive(Debug, Deserialize)]
struct JwkDoc {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Jwk {
    kty: String,
    kid: Option<String>,
    alg: Option<String>,
    r#use: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

async fn jwks_decoding_key(
    validator: &JwtValidator,
    alg: Algorithm,
    uri: String,
    kid_hint: Option<&str>,
) -> Result<DecodingKey, ValidateErr> {
    let keys = validator.load_jwk_entries(&uri).await?;
    let der = pick_jwk_rsa_der(&keys, kid_hint)?;
    let _ = alg; // Alg already matched by decoder; JWKS-derived key is interchangeable for RS*.
    Ok(DecodingKey::from_rsa_der(&der))
}

fn pick_jwk_rsa_der(entries: &[(Option<String>, Vec<u8>)], kid_hint: Option<&str>) -> Result<Vec<u8>, ValidateErr> {
    if let Some(kid) = kid_hint
        && let Some((_, der)) = entries
            .iter()
            .find(|(k, _)| k.as_ref().is_some_and(|x| x == kid))
    {
        return Ok(der.clone());
    }
    if entries.len() == 1 {
        return Ok(entries[0].1.clone());
    }
    Err(ValidateErr::new(
        false,
        "jwt kid did not match any JWKS keys (or JWKS requires kid)".into(),
    ))
}

fn jwk_rsa_to_spki_der(n_b64: &str, e_b64: &str) -> Result<Vec<u8>, String> {
    let n_raw = decode_b64_url(n_b64)?;
    let e_raw = decode_b64_url(e_b64)?;
    let n = BigUint::from_bytes_be(&n_raw);
    let e = BigUint::from_bytes_be(&e_raw);
    let pub_key = RsaPublicKey::new(n, e).map_err(|e| format!("invalid RSA modulus/exponent ({e})"))?;
    pub_key
        .to_public_key_der()
        .map(|d| d.as_bytes().to_vec())
        .map_err(|e| format!("pkcs8 der ({e})"))
}

fn decode_b64_url(s: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|e| format!("base64 ({e})"))
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
            issuers: iss,
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
        let err = validator
            .resolve(None, Some("x"), true)
            .await
            .expect_err("deny");
        assert_eq!(err.code, rpc_codes::AUTH_REQUIRED);
    }

    #[test]
    fn tenant_claim_namespaced_fallback() {
        let claims = serde_json::json!({
            "tenant": "from-plain",
        });
        let t =
            tenant_from_claim_json(&claims, "https://trogon.ai/tenant");
        assert_eq!(t.as_deref(), Some("from-plain"));
    }
}
