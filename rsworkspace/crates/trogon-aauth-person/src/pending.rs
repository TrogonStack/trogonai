//! Token-endpoint state machine per "PS Token Endpoint" (#ps-token-endpoint):
//! deferred responses, concurrent-request correlation, clarification chat,
//! resource-initiated interaction chaining, and re-authorization.

use trogon_identity_types::aauth::person_server::{ClarificationRequired, InteractionRequestType};
use trogon_identity_types::aauth::{AgentClaims, ResourceClaims};

use crate::error::PendingRequestError;
use crate::mission::MissionId;

/// Maximum clarification round-trips per pending request, per "Clarification
/// Limits": "PSes SHOULD enforce a maximum number of clarification rounds
/// (e.g., 5) to prevent indefinite chat loops."
pub const MAX_CLARIFICATION_ROUNDS: u32 = 5;

/// Opaque pending-request identifier, used as the last path segment of the
/// `Location` URL returned on a `202`, per "Pending Response".
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PendingId(pub String);

impl PendingId {
    #[must_use]
    pub fn generate() -> Self {
        Self(uuid_like())
    }
}

impl std::fmt::Display for PendingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Cheap, dependency-free unique id: not a spec-mandated format (the draft
/// leaves pending-id shape to the PS), just needs to be unpredictable and
/// unique per process. Uses a counter + time to avoid pulling in a UUID
/// crate for one call site.
/// CSPRNG-backed: the pending id is the sole credential needed to poll a
/// pending record, including the minted auth token once granted, so it must
/// be unguessable.
fn uuid_like() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Which interaction-shaped state a pending request is waiting in, per
/// "Pending Response" `status` and "Clarification Chat".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingPhase {
    /// Freshly created; policy has not yet run, or policy asked to wait
    /// without further detail (`status: pending`).
    Pending,
    /// Waiting on the user to act on an out-of-band interaction
    /// (`status: interacting`), per "User Interaction".
    Interacting,
    /// Waiting on the agent to answer a clarification question, per
    /// "Clarification Chat".
    AwaitingClarification { clarification: ClarificationRequired },
    /// Waiting on a resource-initiated interaction the PS is relaying, per
    /// "Resource-Initiated Interaction".
    AwaitingResourceInteraction { url: String, code: String },
    /// Approval is pending from a third party, per `requirement=approval`.
    ApprovalPending,
    /// Terminal: granted. Kept in the store briefly so re-polls of the same
    /// pending URL return the same result instead of `NotFound`.
    Granted { auth_token: String, expires_in: i64 },
    /// Terminal: denied.
    Denied { reason: String },
    /// Terminal: canceled by the agent or superseded by a newer request for
    /// the same correlation key. Subsequent polls get `410 Gone`.
    Canceled,
}

/// One in-flight (or recently resolved) token-endpoint request, per
/// "Deferred Responses". Persisted via [`crate::store::PersonStateStore`].
#[derive(Clone, Debug)]
pub struct PendingRequest {
    pub id: PendingId,
    /// Correlation key for "Concurrent Requests": derived from
    /// `(agent.sub, resource.iss, resource.jti)` so repeated requests for the
    /// same outstanding grant land on the same pending entry.
    pub correlation_key: String,
    pub agent: AgentClaims,
    pub resource: ResourceClaims,
    pub justification: Option<String>,
    pub mission_id: Option<MissionId>,
    /// Sub-agent claims from the original request, preserved so a resumed
    /// decision mints against the sub-agent's confirmation key.
    pub subagent: Option<AgentClaims>,
    /// Verified upstream auth token from the original request, preserved so
    /// a resumed decision keeps nesting the delegation chain.
    pub upstream: Option<trogon_aauth_verify::VerifiedAuth>,
    pub phase: PendingPhase,
    pub clarification_round: u32,
}

impl PendingRequest {
    #[must_use]
    pub fn correlation_key_for(agent: &AgentClaims, resource: &ResourceClaims) -> String {
        format!("{}|{}|{}", agent.sub, resource.iss, resource.jti)
    }

    #[must_use]
    pub fn new(agent: AgentClaims, resource: ResourceClaims, justification: Option<String>) -> Self {
        let correlation_key = Self::correlation_key_for(&agent, &resource);
        Self {
            id: PendingId::generate(),
            correlation_key,
            agent,
            resource,
            justification,
            mission_id: None,
            subagent: None,
            upstream: None,
            phase: PendingPhase::Pending,
            clarification_round: 0,
        }
    }

    /// Advances into a clarification round, per "Clarification Chat",
    /// enforcing [`MAX_CLARIFICATION_ROUNDS`].
    pub fn begin_clarification(&mut self, clarification: ClarificationRequired) -> Result<(), PendingRequestError> {
        if self.clarification_round >= MAX_CLARIFICATION_ROUNDS {
            return Err(PendingRequestError::ClarificationLimitExceeded(
                self.id.clone(),
                MAX_CLARIFICATION_ROUNDS,
            ));
        }
        self.clarification_round += 1;
        self.phase = PendingPhase::AwaitingClarification { clarification };
        Ok(())
    }

    pub fn begin_interaction(&mut self) {
        self.phase = PendingPhase::Interacting;
    }

    pub fn begin_resource_interaction(&mut self, url: String, code: String) {
        self.phase = PendingPhase::AwaitingResourceInteraction { url, code };
    }

    pub fn begin_approval_pending(&mut self) {
        self.phase = PendingPhase::ApprovalPending;
    }

    pub fn grant(&mut self, auth_token: String, expires_in: i64) {
        self.phase = PendingPhase::Granted { auth_token, expires_in };
    }

    pub fn deny(&mut self, reason: String) {
        self.phase = PendingPhase::Denied { reason };
    }

    pub fn cancel(&mut self) {
        self.phase = PendingPhase::Canceled;
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.phase,
            PendingPhase::Granted { .. } | PendingPhase::Denied { .. } | PendingPhase::Canceled
        )
    }

    /// Applies the agent's `updated_request` action, per "Updated Request":
    /// the new resource token MUST carry the same `iss`/`agent`/`agent_jkt`
    /// as the original so the identity of the grant target cannot change
    /// mid-flow.
    pub fn apply_updated_resource(&mut self, new_resource: ResourceClaims) -> Result<(), PendingRequestError> {
        if new_resource.iss != self.resource.iss
            || new_resource.agent != self.resource.agent
            || new_resource.agent_jkt != self.resource.agent_jkt
        {
            return Err(PendingRequestError::UpdatedRequestIdentityMismatch);
        }
        self.resource = new_resource;
        self.phase = PendingPhase::Pending;
        Ok(())
    }
}

/// Maps an [`InteractionRequestType`] relayed via the interaction endpoint
/// onto the pending-request transition it drives, per "Resource-Initiated
/// Interaction" and "Interaction Request".
#[must_use]
pub fn interaction_type_starts_resource_interaction(t: &InteractionRequestType) -> bool {
    matches!(t, InteractionRequestType::Interaction | InteractionRequestType::Payment)
}

#[cfg(test)]
mod tests;
