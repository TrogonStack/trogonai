use trogon_registry::AgentCapability;

use crate::event::RouterEvent;

/// Build the system prompt sent to the LLM for a routing decision.
///
/// The prompt lists every registered agent with its capabilities and NATS
/// subject, then shows the incoming event, and asks for a JSON response with
/// `status: "routed" | "unroutable"`.
pub fn build_routing_prompt(event: &RouterEvent, agents: &[AgentCapability]) -> String {
    let mut lines: Vec<String> = Vec::new();

    lines.push("You are the TrogonStack Router Agent. Your job is to decide which Entity Actor should handle an incoming event.".to_string());
    lines.push(String::new());

    // ── Available agents ──────────────────────────────────────────────────────
    if agents.is_empty() {
        lines.push("No agents are currently registered in the system.".to_string());
    } else {
        lines.push("## Registered agents".to_string());
        lines.push(String::new());
        for agent in agents {
            lines.push(format!("### {}", agent.agent_type));
            lines.push(format!("- capabilities: {}", agent.capabilities.join(", ")));
            lines.push(format!("- subject pattern: {}", agent.nats_subject));
            lines.push(format!("- current load: {}", agent.current_load));
            if !agent.metadata.is_null() {
                lines.push(format!("- metadata: {}", agent.metadata));
            }
            lines.push(String::new());
        }
    }

    // ── Incoming event ────────────────────────────────────────────────────────
    lines.push("## Incoming event".to_string());
    lines.push(String::new());
    lines.push(format!("- subject: {}", event.subject));
    lines.push(format!("- event_type: {}", event.event_type()));
    lines.push(String::new());
    lines.push("### Payload (truncated to 4096 bytes)".to_string());
    lines.push("```json".to_string());
    lines.push(event.payload_preview(4096));
    lines.push("```".to_string());
    lines.push(String::new());

    // ── Instructions ─────────────────────────────────────────────────────────
    lines.push("## Task".to_string());
    lines.push(String::new());
    lines.push("Decide which agent should handle this event and what the entity key is.".to_string());
    lines.push("The entity key uniquely identifies the domain object, e.g. `owner/repo/456` for a PR.".to_string());
    lines.push(String::new());
    lines.push("Respond with ONLY a JSON object — no markdown, no commentary.".to_string());
    lines.push(String::new());
    lines.push("If a matching agent exists:".to_string());
    lines.push(r#"{"status":"routed","agent_type":"<agent_type exactly as listed>","entity_key":"<entity key>","reasoning":"<brief explanation>"}"#.to_string());
    lines.push(String::new());
    lines.push("If no agent can handle this event:".to_string());
    lines.push(r#"{"status":"unroutable","reasoning":"<explanation of why no agent matches>"}"#.to_string());

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(subject: &str, payload: &str) -> RouterEvent {
        RouterEvent::new(subject, payload.as_bytes().to_vec())
    }

    fn pr_actor() -> AgentCapability {
        AgentCapability::new(
            "PrActor",
            ["code_review", "security_analysis"],
            "actors.pr.>",
        )
    }

    #[test]
    fn prompt_contains_event_type() {
        let event = make_event("trogon.events.github.pull_request", r#"{"action":"opened"}"#);
        let prompt = build_routing_prompt(&event, &[pr_actor()]);
        assert!(prompt.contains("github.pull_request"));
    }

    #[test]
    fn prompt_contains_agent_info() {
        let event = make_event("trogon.events.github.pull_request", r#"{}"#);
        let prompt = build_routing_prompt(&event, &[pr_actor()]);
        assert!(prompt.contains("PrActor"));
        assert!(prompt.contains("code_review"));
        assert!(prompt.contains("actors.pr.>"));
    }

    #[test]
    fn prompt_contains_json_schema_hint() {
        let event = make_event("trogon.events.github.pull_request", r#"{}"#);
        let prompt = build_routing_prompt(&event, &[pr_actor()]);
        assert!(prompt.contains("\"status\""));
        assert!(prompt.contains("routed"));
        assert!(prompt.contains("unroutable"));
    }

    #[test]
    fn prompt_no_agents_says_so() {
        let event = make_event("trogon.events.unknown.thing", r#"{}"#);
        let prompt = build_routing_prompt(&event, &[]);
        assert!(prompt.contains("No agents are currently registered"));
    }

    #[test]
    fn payload_is_truncated() {
        let big_payload = "x".repeat(10_000);
        let event = make_event("trogon.events.github.push", big_payload.as_str());
        let prompt = build_routing_prompt(&event, &[pr_actor()]);
        assert!(prompt.contains("[truncated]"));
    }
}
