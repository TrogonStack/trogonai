//! `AccessServer`: the AS token endpoint's core state machine, per "AS Token
//! Endpoint" (#as-token-endpoint) and "PS-AS Collapse" (#ps-as-collapse).
//! Library-first and transport-agnostic -- [`crate::http`] adapts this to
//! axum.

use trogon_aauth_verify::TokenVerifier;
use trogon_identity_types::aauth::Act;
use trogon_identity_types::aauth::federation::{AsTokenRequest, AsTokenResponse, ClaimsSubmission};

use crate::error::{AccessServerError, RequestVerificationError};
use crate::mint::{AuthTokenInputs, AuthTokenTtl, mint_auth_jwt, nest_act};
use crate::pending::{PendingRequest, PendingRequestId, PendingRequestStore};
use crate::policy::{Decision, GrantedScope, OrganizationPolicy};
use crate::request::AsTokenContext;
use crate::trust::TrustRegistry;
use crate::verify::{VerifiedRequest, verify_request};

/// Outcome of evaluating one AS token request, per "AS Response"
/// (#as-token-endpoint). Mirrors the three wire shapes the endpoint can
/// return: `200` direct grant, `202`/`requirement=claims`, or a terminal
/// deny mapped to a token endpoint error code by [`crate::http`].
#[derive(Debug)]
pub enum AsOutcome {
    Issued(AsTokenResponse),
    ClaimsRequired {
        pending_id: PendingRequestId,
        required_claims: Vec<String>,
    },
    Denied {
        reason: String,
    },
}

/// Whether this AS is federating with an external PS or collapsed with its
/// own PS role, per "PS-AS Collapse" (#ps-as-collapse). Both modes share the
/// same evaluation and minting logic; the only difference is which `iss` is
/// treated as automatically trusted and which `dwk`-observable identity the
/// resource sees. This crate always mints `dwk: aauth-access.json` (the AS
/// identity) in both modes -- `Collapsed` describes trust, not the minted
/// token's role -- because a resource that chose an AS (`aud` = AS URL, per
/// "PS-AS Collapse" bullet 2) expects an AS-shaped token regardless of
/// whether that AS happens to also be the user's PS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FederationMode {
    /// Standard four-party federation: only PS issuers present in the trust
    /// registry are accepted.
    Federated,
    /// "PS-AS Collapse": this AS is also the agent's PS. `ps_iss` names the
    /// server's own PS-role issuer, which is implicitly trusted without a
    /// trust registry entry -- "federation collapses to a single internal
    /// evaluation... trust between PS and AS is implicit because they are
    /// the same entity."
    Collapsed { ps_iss: String },
}

/// Configuration fixed for the lifetime of an `AccessServer`.
pub struct AccessServerConfig {
    /// This AS's own issuer URL (`aauth-access.json`'s `issuer`), used as the
    /// expected `aud` on inbound resource tokens and the `iss` on minted
    /// auth tokens.
    pub iss: String,
    pub signing_key: jsonwebtoken::EncodingKey,
    pub alg: jsonwebtoken::Algorithm,
    pub kid: String,
    pub auth_token_ttl: AuthTokenTtl,
    pub mode: FederationMode,
}

/// The Access Server core. Generic over the JWKS resolver and clock so
/// production wires HTTP discovery + system time while tests use static
/// fixtures, matching `trogon-aauth-verify`'s `TokenVerifier` pattern.
pub struct AccessServer<R, C, P>
where
    R: trogon_aauth_verify::JwksResolver,
    C: trogon_aauth_verify::TimeSource,
    P: OrganizationPolicy,
{
    verifier: TokenVerifier<R, C>,
    clock: C,
    trust: TrustRegistry,
    policy: P,
    pending: PendingRequestStore,
    cfg: AccessServerConfig,
}

impl<R, C, P> AccessServer<R, C, P>
where
    R: trogon_aauth_verify::JwksResolver,
    C: trogon_aauth_verify::TimeSource + Clone,
    P: OrganizationPolicy,
{
    pub fn new(
        verifier: TokenVerifier<R, C>,
        clock: C,
        mut trust: TrustRegistry,
        policy: P,
        cfg: AccessServerConfig,
    ) -> Self {
        // "PS-AS Collapse": "Trust between PS and AS is implicit because
        // they are the same entity." Seed the registry so `verify_request`'s
        // unconditional trust lookup (shared with `Federated` mode) does not
        // need to know about collapse -- the collapsed PS simply always
        // trusted, with no configuration required.
        if let FederationMode::Collapsed { ps_iss } = &cfg.mode
            && let Ok(iss) = crate::trust::PsIssuer::new(ps_iss.clone())
        {
            trust.trust(iss, crate::trust::TrustBasisRecord::PreEstablished);
        }
        Self {
            verifier,
            clock,
            trust,
            policy,
            pending: PendingRequestStore::new(),
            cfg,
        }
    }

    /// Evaluate a PS-to-AS token request per "AS Token Endpoint"
    /// (#as-token-endpoint). `ps_iss` is the PS issuer that authenticated the
    /// outer HTTP request (verified by the transport layer via
    /// `Signature-Key`/`jwks_uri`, per "PS-to-AS Token Request"); this call
    /// only checks that issuer against the trust registry / collapse mode.
    pub async fn evaluate(&self, ps_iss: &str, req: &AsTokenRequest) -> Result<AsOutcome, AccessServerError> {
        self.require_trusted_or_collapsed(ps_iss)?;

        let (verified, trust) = verify_request(&self.verifier, &self.trust, &self.cfg.iss, ps_iss, req)
            .await
            .map_err(AccessServerError::Verification)?;

        let ctx = self.context_of(&verified);
        let decision = self.policy.decide(&ctx, &trust);
        self.apply_decision(decision, verified, trust)
    }

    /// Resume a request parked on `requirement=claims`, per "Claims
    /// Required" (#requirement-claims): "The recipient MUST provide the
    /// requested claims ... by POSTing to the `Location` URL."
    pub async fn resume_with_claims(
        &self,
        pending_id: &PendingRequestId,
        claims: &ClaimsSubmission,
    ) -> Result<AsOutcome, AccessServerError> {
        let PendingRequest { verified, trust } = self
            .pending
            .take(pending_id)
            .ok_or_else(|| AccessServerError::UnknownPendingRequest(pending_id.as_str().to_string()))?;

        let ctx = self.context_of(&verified);
        let decision = self.policy.decide_with_claims(&ctx, &trust, claims);
        self.apply_decision(decision, verified, trust)
    }

    fn require_trusted_or_collapsed(&self, ps_iss: &str) -> Result<(), AccessServerError> {
        match &self.cfg.mode {
            FederationMode::Collapsed { ps_iss: own_ps_iss } if own_ps_iss == ps_iss => Ok(()),
            _ if self.trust.contains(ps_iss) => Ok(()),
            _ => Err(AccessServerError::Verification(RequestVerificationError::UntrustedPs(
                crate::trust::TrustRegistryError::UnknownIssuer(ps_iss.to_string()),
            ))),
        }
    }

    fn context_of<'a>(&self, verified: &'a VerifiedRequest) -> AsTokenContext<'a> {
        AsTokenContext {
            resource_claims: &verified.resource_claims,
            agent_claims: &verified.agent_claims,
            subagent_claims: verified.subagent_claims.as_ref(),
            upstream: verified.upstream.as_ref(),
            binding: &verified.binding,
        }
    }

    fn apply_decision(
        &self,
        decision: Decision,
        verified: VerifiedRequest,
        trust: crate::trust::TrustedIssuer,
    ) -> Result<AsOutcome, AccessServerError> {
        match decision {
            Decision::Issue { scope } => {
                let token = self.mint(&verified, &scope)?;
                Ok(AsOutcome::Issued(token))
            }
            Decision::RequireClaims { required } => {
                let pending_id = PendingRequestId::new(fresh_pending_id());
                self.pending
                    .insert(pending_id.clone(), PendingRequest { verified, trust });
                Ok(AsOutcome::ClaimsRequired {
                    pending_id,
                    required_claims: required.as_slice().to_vec(),
                })
            }
            Decision::Deny { reason } => Ok(AsOutcome::Denied { reason }),
        }
    }

    fn mint(&self, verified: &VerifiedRequest, scope: &GrantedScope) -> Result<AsTokenResponse, AccessServerError> {
        let act = act_for(verified);
        let sub = subject_for(verified);
        let iat = self.clock.now();
        let jti = fresh_pending_id();

        let inputs = AuthTokenInputs {
            iss: &self.cfg.iss,
            aud: &verified.resource_claims.iss,
            sub: &sub,
            jti: &jti,
            binding: &verified.binding,
            cnf_jwk: cnf_jwk_for(verified),
            scope,
            act,
            principal: None,
            consent_id: None,
            resource: None,
            iat,
            ttl: self.cfg.auth_token_ttl,
        };

        let token = mint_auth_jwt(&self.cfg.signing_key, self.cfg.alg, &self.cfg.kid, &inputs)
            .map_err(AccessServerError::Mint)?;

        Ok(AsTokenResponse {
            auth_token: token,
            expires_in: self.cfg.auth_token_ttl.get(),
        })
    }
}

/// Builds `act` for the minted token, per "Upstream Token Verification"
/// step 4 (call chaining) and "Parent-Mediated Authorization" step 4
/// (sub-agent). Both nest under the same field, but only one applies per
/// request -- a request is either a sub-agent authorization (`subagent_claims`
/// present) or a call-chaining continuation (`upstream` present), not both,
/// per "PS-to-AS Token Request".
fn act_for(verified: &VerifiedRequest) -> Option<Act> {
    // Both cases nest under the *requesting* agent_token's own subject: in
    // call chaining that subject is "the intermediary resource's agent
    // identifier" (step 4); in sub-agent authorization it is the parent
    // agent that mediated on the sub-agent's behalf (#sub-agents step 4).
    verified
        .upstream
        .as_ref()
        .map(|u| nest_act(verified.agent_claims.sub.clone(), u.claims.act.clone()))
        .or_else(|| {
            verified
                .subagent_claims
                .as_ref()
                .map(|_| nest_act(verified.agent_claims.sub.clone(), None))
        })
}

/// Directed `sub` for the minted token. Falls back to the binding agent
/// identifier when no upstream/claims-derived subject exists yet; policies
/// that require a real directed identifier should route through
/// `requirement=claims` before issuing (#requirement-claims), since
/// `AuthClaims.sub` is non-optional in the current type -- see promotion
/// note in `lib.rs`.
fn subject_for(verified: &VerifiedRequest) -> String {
    verified
        .upstream
        .as_ref()
        .map(|u| u.claims.sub.clone())
        .unwrap_or_else(|| verified.binding.agent.clone())
}

fn cnf_jwk_for(verified: &VerifiedRequest) -> serde_json::Value {
    verified
        .subagent_claims
        .as_ref()
        .unwrap_or(&verified.agent_claims)
        .cnf
        .jwk
        .clone()
}

fn fresh_pending_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("as-{nanos:x}")
}

#[cfg(test)]
mod tests;
