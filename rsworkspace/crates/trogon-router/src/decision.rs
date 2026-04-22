use serde::{Deserialize, Serialize};

/// The outcome of a routing attempt for a single event.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteResult {
    /// The LLM matched the event to a registered agent.
    Routed(RoutingDecision),
    /// The LLM could not find a matching agent for this event.
    /// The reasoning is recorded in the transcript to inform which new
    /// Entity Actor type should be built.
    Unroutable { reasoning: String },
}

/// A successful routing decision produced by the LLM.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingDecision {
    /// The `agent_type` from the registry — exactly as registered.
    pub agent_type: String,
    /// The entity-specific key, e.g. `"owner/repo/456"`.
    pub entity_key: String,
    /// The LLM's explanation of why it chose this agent.
    pub reasoning: String,
}

/// Intermediate JSON structure used to deserialize the raw LLM response.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LlmRoutingResponse {
    Routed {
        agent_type: String,
        entity_key: String,
        reasoning: String,
    },
    Unroutable {
        reasoning: String,
    },
}

impl From<LlmRoutingResponse> for RouteResult {
    fn from(r: LlmRoutingResponse) -> Self {
        match r {
            LlmRoutingResponse::Routed {
                agent_type,
                entity_key,
                reasoning,
            } => RouteResult::Routed(RoutingDecision {
                agent_type,
                entity_key,
                reasoning,
            }),
            LlmRoutingResponse::Unroutable { reasoning } => RouteResult::Unroutable { reasoning },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_routed_response() {
        let json = r#"{"status":"routed","agent_type":"PrActor","entity_key":"owner/repo/1","reasoning":"PR event"}"#;
        let r: LlmRoutingResponse = serde_json::from_str(json).unwrap();
        let result: RouteResult = r.into();
        assert_eq!(
            result,
            RouteResult::Routed(RoutingDecision {
                agent_type: "PrActor".into(),
                entity_key: "owner/repo/1".into(),
                reasoning: "PR event".into(),
            })
        );
    }

    #[test]
    fn deserialize_unroutable_response() {
        let json = r#"{"status":"unroutable","reasoning":"no matching agent"}"#;
        let r: LlmRoutingResponse = serde_json::from_str(json).unwrap();
        let result: RouteResult = r.into();
        assert_eq!(
            result,
            RouteResult::Unroutable {
                reasoning: "no matching agent".into()
            }
        );
    }
}
