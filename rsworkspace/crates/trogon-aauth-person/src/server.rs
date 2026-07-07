//! Transport-agnostic Person Server core, per draft section "Person Server"
//! (#person-server). Wires together request verification
//! ([`crate::agent`]), person policy ([`crate::decision`]), interaction
//! delivery ([`crate::interaction`]), state persistence ([`crate::store`]),
//! and token minting ([`crate::mint`]) behind one facade that
//! `crate::http` binds to axum.

use jsonwebtoken::{Algorithm, EncodingKey};
use trogon_aauth_verify::{JwksResolver, TimeSource, TokenVerifier};
use trogon_identity_types::aauth::mission::{MissionBlob, MissionLogEntry};
use trogon_identity_types::aauth::person_server::{
    ClarificationAction, ClarificationRequired, PendingResponse, PendingStatus, TokenGrantResponse, TokenRequest,
    UpdatedRequest,
};

use crate::agent::verify_request;
use crate::decision::{DecisionRequest, PolicyDecision, PolicyEngine};
use crate::error::{PendingRequestError, PersonServerError};
use crate::interaction::{InteractionChannel, InteractionNotice};
use crate::mint::{AuthTokenInputs, AuthTokenTtl, mint_auth_jwt, nest_act};
use crate::mission::{Mission, MissionContext, MissionId};
use crate::pending::{PendingId, PendingRequest};
use crate::store::PersonStateStore;

/// Result of evaluating a token-endpoint request, mapped to HTTP by
/// `crate::http`: either an immediate `200` grant or a `202` pending
/// response the agent must poll.
#[derive(Debug)]
pub enum TokenEndpointOutcome {
    Grant {
        response: TokenGrantResponse,
    },
    Pending {
        pending_id: PendingId,
        response: PendingResponse,
    },
}

/// The Person Server core. Generic over the JWKS resolver and clock (both
/// reused from `trogon-aauth-verify`) plus the three deployment-specific
/// seams: policy, interaction delivery, and state persistence.
pub struct PersonServer<R, C, P, I, S>
where
    R: JwksResolver,
    C: TimeSource + Clone,
    P: PolicyEngine,
    I: InteractionChannel,
    S: PersonStateStore,
{
    verifier: TokenVerifier<R, C>,
    clock: C,
    policy: P,
    interaction: I,
    store: S,
    signing_key: EncodingKey,
    alg: Algorithm,
    kid: String,
    /// This PS's own issuer URL, per "Auth Token Structure" `iss` and "PS
    /// Response" (resource token `aud` must equal this).
    iss: String,
}

impl<R, C, P, I, S> PersonServer<R, C, P, I, S>
where
    R: JwksResolver,
    C: TimeSource + Clone,
    P: PolicyEngine,
    I: InteractionChannel,
    S: PersonStateStore,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        verifier: TokenVerifier<R, C>,
        clock: C,
        policy: P,
        interaction: I,
        store: S,
        signing_key: EncodingKey,
        alg: Algorithm,
        kid: impl Into<String>,
        iss: impl Into<String>,
    ) -> Self {
        Self {
            verifier,
            clock,
            policy,
            interaction,
            store,
            signing_key,
            alg,
            kid: kid.into(),
            iss: iss.into(),
        }
    }

    #[must_use]
    pub fn iss(&self) -> &str {
        &self.iss
    }

    /// "PS Token Endpoint" (#ps-token-endpoint): verifies the request, then
    /// either resumes an existing pending flow for the same
    /// agent+resource pair ("Concurrent Requests") or runs policy fresh.
    pub async fn evaluate_token_request(
        &self,
        agent_token: &str,
        req: &TokenRequest,
    ) -> Result<TokenEndpointOutcome, PersonServerError> {
        let verified = verify_request(&self.verifier, &self.iss, agent_token, req)
            .await
            .map_err(PersonServerError::Verification)?;

        let correlation_key = PendingRequest::correlation_key_for(&verified.agent_claims, &verified.resource_claims);
        if let Some(existing_id) = self
            .store
            .find_pending_by_correlation(&correlation_key)
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?
            && let Some(existing) = self
                .store
                .get_pending(&existing_id)
                .await
                .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?
            && !existing.is_terminal()
        {
            return Ok(TokenEndpointOutcome::Pending {
                pending_id: existing.id.clone(),
                response: PendingResponse::pending(),
            });
        }

        let mission_id = verified
            .resource_claims
            .mission
            .as_ref()
            .map(|m| MissionId(m.s256.clone()));
        let mission_ctx = self.load_mission_context(mission_id.as_ref()).await?;

        let mut pending = PendingRequest::new(
            verified.agent_claims.clone(),
            verified.resource_claims.clone(),
            verified.justification.clone(),
        );
        pending.mission_id = mission_id.clone();

        let decision = self
            .policy
            .decide(DecisionRequest {
                agent: &verified.agent_claims,
                resource: &verified.resource_claims,
                justification: verified.justification.as_deref(),
                mission: mission_ctx.as_ref(),
                clarification_round: 0,
            })
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.to_string())))?;

        self.apply_decision(&mut pending, decision, &verified, mission_id.as_ref())
            .await
    }

    /// Applies a policy decision to a (fresh or resumed) pending request,
    /// minting a grant, persisting the new pending state, or relaying an
    /// interaction, per "PS Response" / "User Interaction" / "Clarification
    /// Chat".
    async fn apply_decision(
        &self,
        pending: &mut PendingRequest,
        decision: PolicyDecision,
        verified: &crate::agent::VerifiedRequest,
        mission_id: Option<&MissionId>,
    ) -> Result<TokenEndpointOutcome, PersonServerError> {
        match decision {
            PolicyDecision::Grant { scope } => {
                let act = verified
                    .upstream
                    .as_ref()
                    .map(|u| nest_act(verified.agent_claims.sub.clone(), u.claims.act.clone()));
                let iat = self.clock.now();
                let jti = crate::pending::PendingId::generate().0;
                let inputs = AuthTokenInputs {
                    iss: &self.iss,
                    aud: &verified.resource_claims.iss,
                    sub: &verified.binding.agent,
                    jti: &jti,
                    binding: &verified.binding,
                    cnf_jwk: verified.agent_claims.cnf.jwk.clone(),
                    scope: &scope,
                    act,
                    principal: None,
                    consent_id: None,
                    resource: Some(&verified.resource_claims.iss),
                    iat,
                    ttl: AuthTokenTtl::default(),
                };
                let auth_token =
                    mint_auth_jwt(&self.signing_key, self.alg, &self.kid, &inputs).map_err(PersonServerError::Mint)?;
                let expires_in = inputs.ttl.get();
                pending.grant(auth_token.clone(), expires_in);
                self.persist(
                    pending,
                    mission_id,
                    MissionLogEntry::TokenRequest {
                        justification: pending.justification.clone(),
                    },
                )
                .await?;
                Ok(TokenEndpointOutcome::Grant {
                    response: TokenGrantResponse { auth_token, expires_in },
                })
            }
            PolicyDecision::Deny { reason } => {
                pending.deny(reason.clone());
                self.persist(
                    pending,
                    mission_id,
                    MissionLogEntry::TokenRequest {
                        justification: pending.justification.clone(),
                    },
                )
                .await?;
                Err(PersonServerError::Denied(reason))
            }
            PolicyDecision::NeedsInteraction => {
                let notice = InteractionNotice {
                    url: format!("https://ps.invalid/interact/{}", pending.id.0),
                    code: pending.id.0.clone(),
                    description: pending.justification.clone(),
                };
                match self.interaction.notify(&notice).await {
                    Ok(()) => pending.begin_interaction(),
                    Err(crate::error::InteractionRelayError::Unavailable) => pending.begin_interaction(),
                    Err(crate::error::InteractionRelayError::UserUnreachable) => {
                        return Err(PersonServerError::UserUnreachable);
                    }
                }
                self.store
                    .insert_pending(pending.clone())
                    .await
                    .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?;
                Ok(TokenEndpointOutcome::Pending {
                    pending_id: pending.id.clone(),
                    response: PendingResponse {
                        status: PendingStatus::Interacting.as_str().to_string(),
                        ..PendingResponse::pending()
                    },
                })
            }
            PolicyDecision::NeedsClarification { clarification, options } => {
                let body = ClarificationRequired {
                    status: PendingStatus::Pending.as_str().to_string(),
                    clarification: clarification.clone(),
                    timeout: None,
                    options: options.clone(),
                };
                pending.begin_clarification(body).map_err(PersonServerError::Pending)?;
                self.store
                    .insert_pending(pending.clone())
                    .await
                    .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?;
                Ok(TokenEndpointOutcome::Pending {
                    pending_id: pending.id.clone(),
                    response: PendingResponse {
                        status: PendingStatus::Pending.as_str().to_string(),
                        clarification: Some(clarification),
                        timeout: None,
                        options,
                        required_claims: None,
                    },
                })
            }
            PolicyDecision::ApprovalPending => {
                pending.begin_approval_pending();
                self.store
                    .insert_pending(pending.clone())
                    .await
                    .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?;
                Ok(TokenEndpointOutcome::Pending {
                    pending_id: pending.id.clone(),
                    response: PendingResponse::pending(),
                })
            }
        }
    }

    async fn persist(
        &self,
        pending: &PendingRequest,
        mission_id: Option<&MissionId>,
        log_entry: MissionLogEntry,
    ) -> Result<(), PersonServerError> {
        self.store
            .insert_pending(pending.clone())
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?;
        if let Some(id) = mission_id
            && let Some(mut mission) = self
                .store
                .get_mission(id)
                .await
                .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?
        {
            mission.append_log(log_entry);
            self.store
                .update_mission(mission)
                .await
                .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?;
        }
        Ok(())
    }

    async fn load_mission_context(
        &self,
        mission_id: Option<&MissionId>,
    ) -> Result<Option<MissionContext>, PersonServerError> {
        let Some(id) = mission_id else { return Ok(None) };
        let mission = self
            .store
            .get_mission(id)
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?
            .ok_or_else(|| PersonServerError::MissionNotFound(id.0.clone()))?;
        if !mission.is_active() {
            return Err(PersonServerError::MissionNotActive(
                id.0.clone(),
                mission.status.clone(),
            ));
        }
        Ok(Some(MissionContext::from(&mission)))
    }

    /// "Deferred Responses" polling: returns the current state of a pending
    /// request without mutating it.
    pub async fn poll_pending(&self, id: &PendingId) -> Result<PendingRequest, PersonServerError> {
        self.store
            .get_pending(id)
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?
            .ok_or_else(|| PersonServerError::Pending(PendingRequestError::NotFound(id.0.clone())))
    }

    /// "Agent Response to Clarification": the agent replies with either a
    /// [`ClarificationAction::ClarificationResponse`] (free-text answer,
    /// re-runs policy) or [`ClarificationAction::UpdatedRequest`] (a revised
    /// resource token, re-verified before policy re-runs).
    pub async fn respond_to_clarification(
        &self,
        id: &PendingId,
        action: ClarificationAction,
        clarification_response: Option<&str>,
        updated_request: Option<&UpdatedRequest>,
    ) -> Result<TokenEndpointOutcome, PersonServerError> {
        let mut pending = self
            .store
            .get_pending(id)
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?
            .ok_or_else(|| PersonServerError::Pending(PendingRequestError::NotFound(id.0.clone())))?;

        if pending.is_terminal() {
            return Err(PersonServerError::Pending(PendingRequestError::Gone(id.0.clone())));
        }

        match action {
            ClarificationAction::ClarificationResponse => {
                let response_text = clarification_response.unwrap_or_default();
                let mut justification = pending.justification.clone().unwrap_or_default();
                if !justification.is_empty() {
                    justification.push('\n');
                }
                justification.push_str(response_text);
                pending.justification = Some(justification);
            }
            ClarificationAction::UpdatedRequest => {
                let updated = updated_request.ok_or(PersonServerError::Pending(
                    PendingRequestError::UpdatedRequestIdentityMismatch,
                ))?;
                let verified_resource = self
                    .verifier
                    .verify_resource(&updated.resource_token, &self.iss)
                    .await
                    .map_err(crate::error::RequestVerificationError::ResourceToken)
                    .map_err(PersonServerError::Verification)?;
                pending
                    .apply_updated_resource(verified_resource.claims)
                    .map_err(PersonServerError::Pending)?;
                if let Some(j) = &updated.justification {
                    pending.justification = Some(j.clone());
                }
            }
        }

        let mission_ctx = self.load_mission_context(pending.mission_id.as_ref()).await?;
        let decision = self
            .policy
            .decide(DecisionRequest {
                agent: &pending.agent,
                resource: &pending.resource,
                justification: pending.justification.as_deref(),
                mission: mission_ctx.as_ref(),
                clarification_round: pending.clarification_round,
            })
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.to_string())))?;

        let binding = crate::mint::BindingAgent {
            agent: pending.resource.agent.clone(),
            agent_jkt: pending.resource.agent_jkt.clone(),
        };
        let verified = crate::agent::VerifiedRequest {
            resource_claims: pending.resource.clone(),
            agent_claims: pending.agent.clone(),
            subagent_claims: None,
            upstream: None,
            binding,
            justification: pending.justification.clone(),
        };
        let mission_id = pending.mission_id.clone();
        self.apply_decision(&mut pending, decision, &verified, mission_id.as_ref())
            .await
    }

    /// "Mission Approval": approves a mission proposal and persists the
    /// byte-exact blob for later `s256` verification.
    pub async fn approve_mission(&self, blob: MissionBlob) -> Result<Vec<u8>, PersonServerError> {
        let blob_bytes = serde_json::to_vec(&blob)
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.to_string())))?;
        let mission = Mission::approve(blob_bytes.clone(), blob);
        self.store
            .insert_mission(mission)
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?;
        Ok(blob_bytes)
    }

    /// "Mission Completion" / "Mission Management": terminates an active
    /// mission. Missions have exactly two states; termination is one-way.
    pub async fn complete_mission(&self, id: &MissionId) -> Result<(), PersonServerError> {
        let mut mission = self
            .store
            .get_mission(id)
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?
            .ok_or_else(|| PersonServerError::MissionNotFound(id.0.clone()))?;
        mission.complete();
        self.store
            .update_mission(mission)
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?;
        Ok(())
    }

    pub async fn get_mission(&self, id: &MissionId) -> Result<Option<Mission>, PersonServerError> {
        self.store
            .get_mission(id)
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))
    }

    /// "Permission Endpoint" / "Audit Endpoint": appends one governed event
    /// to the referenced mission's log, per "Mission Log".
    pub async fn append_mission_log(
        &self,
        mission_id: &MissionId,
        entry: MissionLogEntry,
    ) -> Result<(), PersonServerError> {
        let mut mission = self
            .store
            .get_mission(mission_id)
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?
            .ok_or_else(|| PersonServerError::MissionNotFound(mission_id.0.clone()))?;
        if !mission.is_active() {
            return Err(PersonServerError::MissionNotActive(
                mission_id.0.clone(),
                mission.status.clone(),
            ));
        }
        mission.append_log(entry);
        self.store
            .update_mission(mission)
            .await
            .map_err(|e| PersonServerError::Pending(PendingRequestError::NotFound(e.0)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
