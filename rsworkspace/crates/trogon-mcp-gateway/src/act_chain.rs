//! Shadow-mode projection of the `mcp-act-chain` NATS header.

use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::HeaderMap;
use tracing::warn;

pub use trogon_identity_types::{ActChainEntry, MAX_ACT_CHAIN_DEPTH, parse_act_chain};

pub use crate::agent_identity::AgentIdentityMode;

pub const MCP_ACT_CHAIN_HEADER: &str = "mcp-act-chain";

const ENV_GATEWAY_IDENTITY_SUB: &str = "MCP_GATEWAY_IDENTITY_SUB";
const ENV_GATEWAY_IDENTITY_AGENT_ID: &str = "MCP_GATEWAY_IDENTITY_AGENT_ID";
const ENV_GATEWAY_IDENTITY_WKL: &str = "MCP_GATEWAY_IDENTITY_WKL";
const DEFAULT_GATEWAY_IDENTITY_SUB: &str = "trogon-mcp-gateway";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActChainProjectOutcome {
    NoOp,
    Projected { depth: usize },
    TooDeepForwarded { depth: usize },
}

fn gateway_identity_sub_from_env() -> String {
    std::env::var(ENV_GATEWAY_IDENTITY_SUB)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GATEWAY_IDENTITY_SUB.to_string())
}

fn gateway_identity_field_from_env(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|s| !s.trim().is_empty())
}

fn current_unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn ingress_act_chain_raw(headers: Option<&HeaderMap>) -> Option<String> {
    let hm = headers?;
    hm.get_last(MCP_ACT_CHAIN_HEADER)
        .or_else(|| hm.get(MCP_ACT_CHAIN_HEADER))
        .map(|v| v.as_str().to_string())
}

pub fn project_act_chain_header(headers: &mut HeaderMap, ingress_raw: Option<&str>, mode: AgentIdentityMode) {
    let gateway_sub = gateway_identity_sub_from_env();
    let gateway_agent_id = gateway_identity_field_from_env(ENV_GATEWAY_IDENTITY_AGENT_ID);
    let gateway_wkl = gateway_identity_field_from_env(ENV_GATEWAY_IDENTITY_WKL);
    let now_iat = current_unix_time();
    let _ = project_act_chain_header_inner(
        headers,
        ingress_raw,
        mode,
        gateway_sub.as_str(),
        gateway_agent_id.as_deref(),
        gateway_wkl.as_deref(),
        now_iat,
    );
}

pub(crate) fn project_act_chain_header_inner(
    headers: &mut HeaderMap,
    ingress_raw: Option<&str>,
    mode: AgentIdentityMode,
    gateway_sub: &str,
    gateway_agent_id: Option<&str>,
    gateway_wkl: Option<&str>,
    now_iat: i64,
) -> ActChainProjectOutcome {
    if mode == AgentIdentityMode::Off {
        return ActChainProjectOutcome::NoOp;
    }

    let mut chain = match ingress_raw {
        Some(raw) if !raw.trim().is_empty() => match parse_act_chain(raw) {
            Ok(entries) => entries,
            Err(e) => {
                warn!(event = "act_chain_parse_error", error = %e, "failed to parse mcp-act-chain header");
                Vec::new()
            }
        },
        _ => Vec::new(),
    };

    if chain.len() >= MAX_ACT_CHAIN_DEPTH {
        warn!(
            event = "act_chain_too_deep",
            depth = chain.len(),
            max_depth = MAX_ACT_CHAIN_DEPTH,
            "act chain exceeds max depth"
        );
        if !chain.is_empty() {
            set_act_chain_header(headers, &chain);
        }
        return ActChainProjectOutcome::TooDeepForwarded { depth: chain.len() };
    }

    chain.push(ActChainEntry {
        sub: gateway_sub.to_string(),
        agent_id: gateway_agent_id.map(str::to_string),
        wkl: gateway_wkl.map(str::to_string),
        iat: now_iat,
    });
    let depth = chain.len();
    set_act_chain_header(headers, &chain);
    ActChainProjectOutcome::Projected { depth }
}

fn set_act_chain_header(headers: &mut HeaderMap, chain: &[ActChainEntry]) {
    if let Ok(json) = serde_json::to_string(chain) {
        headers.insert(MCP_ACT_CHAIN_HEADER, json.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(sub: &str, iat: i64) -> ActChainEntry {
        ActChainEntry {
            sub: sub.to_string(),
            agent_id: None,
            wkl: None,
            iat,
        }
    }

    fn chain_json(entries: &[ActChainEntry]) -> String {
        serde_json::to_string(entries).unwrap()
    }

    fn header_chain(headers: &HeaderMap) -> Option<Vec<ActChainEntry>> {
        headers
            .get(MCP_ACT_CHAIN_HEADER)
            .map(|v| parse_act_chain(v.as_str()).unwrap())
    }

    #[test]
    fn mode_off_leaves_header_absent() {
        let mut headers = HeaderMap::new();
        let outcome = project_act_chain_header_inner(
            &mut headers,
            None,
            AgentIdentityMode::Off,
            "trogon-mcp-gateway",
            None,
            None,
            1,
        );
        assert_eq!(outcome, ActChainProjectOutcome::NoOp);
        assert!(headers.get(MCP_ACT_CHAIN_HEADER).is_none());
    }

    #[test]
    fn empty_chain_shadow_appends_gateway_entry() {
        let mut headers = HeaderMap::new();
        let outcome = project_act_chain_header_inner(
            &mut headers,
            None,
            AgentIdentityMode::Shadow,
            "trogon-mcp-gateway",
            None,
            None,
            1_700_000_000,
        );
        assert_eq!(outcome, ActChainProjectOutcome::Projected { depth: 1 });
        let chain = header_chain(&headers).unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].sub, "trogon-mcp-gateway");
        assert_eq!(chain[0].iat, 1_700_000_000);
        assert!(chain[0].agent_id.is_none());
        assert!(chain[0].wkl.is_none());
    }

    #[test]
    fn registered_gateway_identity_populates_agent_id_and_wkl() {
        let mut headers = HeaderMap::new();
        let outcome = project_act_chain_header_inner(
            &mut headers,
            None,
            AgentIdentityMode::Shadow,
            "agent:trogon/gateway-prod-1",
            Some("trogon/gateway-prod-1"),
            Some("spiffe://trogon.local/ns/prod/sa/mcp-gateway"),
            1_700_000_000,
        );
        assert_eq!(outcome, ActChainProjectOutcome::Projected { depth: 1 });
        let chain = header_chain(&headers).unwrap();
        assert_eq!(chain[0].sub, "agent:trogon/gateway-prod-1");
        assert_eq!(chain[0].agent_id.as_deref(), Some("trogon/gateway-prod-1"));
        assert_eq!(
            chain[0].wkl.as_deref(),
            Some("spiffe://trogon.local/ns/prod/sa/mcp-gateway")
        );
    }

    #[test]
    fn existing_two_entry_chain_shadow_becomes_three() {
        let existing = vec![sample_entry("user-a", 100), sample_entry("agent-b", 200)];
        let raw = chain_json(&existing);
        let mut headers = HeaderMap::new();
        let outcome = project_act_chain_header_inner(
            &mut headers,
            Some(raw.as_str()),
            AgentIdentityMode::Shadow,
            "trogon-mcp-gateway",
            None,
            None,
            300,
        );
        assert_eq!(outcome, ActChainProjectOutcome::Projected { depth: 3 });
        let chain = header_chain(&headers).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].sub, "user-a");
        assert_eq!(chain[1].sub, "agent-b");
        assert_eq!(chain[2].sub, "trogon-mcp-gateway");
        assert_eq!(chain[2].iat, 300);
    }

    #[test]
    fn depth_at_max_shadow_forwards_without_append() {
        let existing: Vec<ActChainEntry> = (0..MAX_ACT_CHAIN_DEPTH)
            .map(|i| sample_entry(&format!("hop-{i}"), i as i64))
            .collect();
        let raw = chain_json(&existing);
        let mut headers = HeaderMap::new();
        let outcome = project_act_chain_header_inner(
            &mut headers,
            Some(raw.as_str()),
            AgentIdentityMode::Shadow,
            "trogon-mcp-gateway",
            None,
            None,
            999,
        );
        assert_eq!(
            outcome,
            ActChainProjectOutcome::TooDeepForwarded {
                depth: MAX_ACT_CHAIN_DEPTH
            }
        );
        let chain = header_chain(&headers).unwrap();
        assert_eq!(chain.len(), MAX_ACT_CHAIN_DEPTH);
        assert_eq!(chain.last().unwrap().sub, format!("hop-{}", MAX_ACT_CHAIN_DEPTH - 1));
    }

    #[test]
    fn parse_error_treated_as_absent_in_shadow_still_appends_gateway() {
        let mut headers = HeaderMap::new();
        let outcome = project_act_chain_header_inner(
            &mut headers,
            Some("not-json"),
            AgentIdentityMode::Shadow,
            "trogon-mcp-gateway",
            None,
            None,
            42,
        );
        assert_eq!(outcome, ActChainProjectOutcome::Projected { depth: 1 });
        let chain = header_chain(&headers).unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].sub, "trogon-mcp-gateway");
    }
}
