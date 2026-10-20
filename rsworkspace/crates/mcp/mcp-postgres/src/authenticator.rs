use crate::authentication_config::Issuer;
use crate::{AuthError, AuthenticationConfigInput, DelegationPolicy, RequestIdentity};
use jsonwebtoken::jwk::{Jwk, JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct Authenticator {
    issuers: BTreeMap<String, IssuerState>,
    client: reqwest::Client,
}

struct IssuerState {
    config: Issuer,
    keys: Mutex<KeyCache>,
}

struct KeyCache {
    keys: JwkSet,
    fetched: Instant,
    attempted: Option<Instant>,
}

impl Authenticator {
    pub async fn new(input: AuthenticationConfigInput) -> Result<Self, AuthError> {
        if input.issuers.is_empty() {
            return Err(AuthError::Configuration);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| AuthError::Configuration)?;
        let mut issuers = BTreeMap::new();
        for input in input.issuers {
            let config = Issuer::try_from(input)?;
            if issuers.contains_key(&config.issuer) {
                return Err(AuthError::Configuration);
            }
            let keys = fetch_keys(&client, &config).await?;
            let now = Instant::now();
            issuers.insert(
                config.issuer.clone(),
                IssuerState {
                    config,
                    keys: Mutex::new(KeyCache {
                        keys,
                        fetched: now,
                        attempted: None,
                    }),
                },
            );
        }
        Ok(Self { issuers, client })
    }

    pub fn issuer_urls(&self) -> Vec<String> {
        self.issuers.keys().cloned().collect()
    }

    pub async fn authenticate(&self, token: &str) -> Result<RequestIdentity, AuthError> {
        if token.len() > crate::constants::MAX_TOKEN_BYTES {
            return Err(AuthError::Unauthenticated);
        }
        let unverified =
            jsonwebtoken::dangerous::insecure_decode::<Value>(token).map_err(|_| AuthError::Unauthenticated)?;
        let issuer = string_claim(&unverified.claims, "iss")?;
        let state = self.issuers.get(issuer).ok_or(AuthError::Unauthenticated)?;
        let header = decode_header(token).map_err(|_| AuthError::Unauthenticated)?;
        if header.crit.is_some() || header.enc.is_some() || header.zip.is_some() {
            return Err(AuthError::Unauthenticated);
        }
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256 | Algorithm::EdDSA) {
            return Err(AuthError::Unauthenticated);
        }
        let kid = header
            .kid
            .as_deref()
            .filter(|id| !id.is_empty())
            .ok_or(AuthError::Unauthenticated)?;
        let key = self.key(state, kid, header.alg).await?;
        let mut validation = Validation::new(header.alg);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.set_issuer(&[&state.config.issuer]);
        validation.set_audience(&[&state.config.audience]);
        validation.validate_nbf = true;
        validation.leeway = 0;
        let claims = decode::<Value>(token, &key, &validation)
            .map_err(|_| AuthError::Unauthenticated)?
            .claims;
        principal(&state.config, &claims)
    }

    async fn key(&self, state: &IssuerState, kid: &str, algorithm: Algorithm) -> Result<DecodingKey, AuthError> {
        let mut cache = state.keys.lock().await;
        let expired = cache.fetched.elapsed() >= Duration::from_secs(crate::constants::JWKS_CACHE_SECONDS);
        let missing = !cache
            .keys
            .keys
            .iter()
            .any(|key| key.common.key_id.as_deref() == Some(kid));
        if expired || missing {
            if cache.attempted.is_some_and(|attempted| {
                attempted.elapsed() < Duration::from_secs(crate::constants::JWKS_REFRESH_SECONDS)
            }) {
                return Err(if expired {
                    AuthError::Unavailable
                } else {
                    AuthError::Unauthenticated
                });
            }
            cache.attempted = Some(Instant::now());
            cache.keys = fetch_keys(&self.client, &state.config).await?;
            cache.fetched = Instant::now();
        }
        let mut matches = cache
            .keys
            .keys
            .iter()
            .filter(|key| key.common.key_id.as_deref() == Some(kid));
        let jwk = matches.next().ok_or(AuthError::Unauthenticated)?;
        if matches.next().is_some() || !compatible(jwk, algorithm) {
            return Err(AuthError::Unauthenticated);
        }
        DecodingKey::from_jwk(jwk).map_err(|_| AuthError::Unauthenticated)
    }
}

async fn fetch_keys(client: &reqwest::Client, issuer: &Issuer) -> Result<JwkSet, AuthError> {
    let mut response = client
        .get(issuer.jwks_uri.clone())
        .send()
        .await
        .map_err(|_| AuthError::Unavailable)?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > crate::constants::MAX_JWKS_BYTES as u64)
    {
        return Err(AuthError::Unavailable);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Unavailable)? {
        if bytes.len().saturating_add(chunk.len()) > crate::constants::MAX_JWKS_BYTES {
            return Err(AuthError::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    let keys: JwkSet = serde_json::from_slice(&bytes).map_err(|_| AuthError::Unavailable)?;
    if keys.keys.is_empty() {
        return Err(AuthError::Unavailable);
    }
    Ok(keys)
}

fn compatible(key: &Jwk, algorithm: Algorithm) -> bool {
    key.common
        .public_key_use
        .as_ref()
        .is_none_or(|usage| *usage == PublicKeyUse::Signature)
        && key
            .common
            .key_operations
            .as_ref()
            .is_none_or(|operations| operations.contains(&KeyOperations::Verify))
        && key
            .common
            .key_algorithm
            .is_none_or(|alg| alg == KeyAlgorithm::from(algorithm))
}

fn principal(config: &Issuer, claims: &Value) -> Result<RequestIdentity, AuthError> {
    string_claim(claims, "sub")?;
    if claims.get("chain_act").is_some() {
        return Err(AuthError::Unauthenticated);
    }
    let user = config
        .users
        .get(string_claim(claims, &config.user_claim)?)
        .ok_or(AuthError::Unauthenticated)?
        .clone();
    let scopes: BTreeSet<&str> = match claims.get(&config.scope_claim) {
        Some(Value::String(scope)) => scope.split_whitespace().collect(),
        Some(Value::Array(scopes)) => scopes
            .iter()
            .map(|scope| scope.as_str().ok_or(AuthError::Unauthenticated))
            .collect::<Result<_, _>>()?,
        None if config.required_scopes.is_empty() => BTreeSet::new(),
        _ => return Err(AuthError::Unauthenticated),
    };
    if config
        .required_scopes
        .iter()
        .any(|scope| !scopes.contains(scope.as_str()))
    {
        return Err(AuthError::InsufficientScope);
    }
    let act = claims.get("act");
    if (act.is_some() && config.delegation == DelegationPolicy::Forbidden)
        || (act.is_none() && config.delegation == DelegationPolicy::Required)
    {
        return Err(AuthError::Unauthenticated);
    }
    let mut actors = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = act;
    while let Some(actor) = current {
        if actors.len() >= crate::constants::MAX_DELEGATION_DEPTH || !actor.is_object() {
            return Err(AuthError::Unauthenticated);
        }
        let external = string_claim(actor, "sub")?;
        if actor
            .get("iss")
            .is_some_and(|issuer| issuer.as_str() != Some(config.issuer.as_str()))
        {
            return Err(AuthError::Unauthenticated);
        }
        let mapped = config.clients.get(external).ok_or(AuthError::Unauthenticated)?.clone();
        if !seen.insert(mapped.clone()) {
            return Err(AuthError::Unauthenticated);
        }
        actors.push(mapped);
        current = actor.get("act");
    }
    let client = match actors.first() {
        Some(actor) => actor.clone(),
        None => config
            .clients
            .get(string_claim(claims, &config.client_claim)?)
            .ok_or(AuthError::Unauthenticated)?
            .clone(),
    };
    Ok(RequestIdentity::new(user, client, actors))
}

fn string_claim<'a>(claims: &'a Value, name: &str) -> Result<&'a str, AuthError> {
    claims
        .get(name)
        .and_then(Value::as_str)
        .filter(|claim| !claim.is_empty())
        .ok_or(AuthError::Unauthenticated)
}

#[cfg(test)]
#[path = "authenticator_tests.rs"]
mod tests;
