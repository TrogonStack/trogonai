use serde::{Deserialize, Serialize};

pub const MAX_ACT_CHAIN_DEPTH: usize = 8;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ActChainEntry {
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wkl: Option<String>,
    pub iat: i64,
}

pub fn parse_act_chain(raw: &str) -> Result<Vec<ActChainEntry>, serde_json::Error> {
    serde_json::from_str(raw)
}

/// Returns true when `(agent_id, wkl)` appears more than once in the chain.
pub fn act_chain_has_loop(chain: &[ActChainEntry]) -> bool {
    let mut seen = std::collections::HashSet::new();
    for entry in chain {
        let agent = entry.agent_id.as_deref().unwrap_or("");
        let wkl = entry.wkl.as_deref().unwrap_or("");
        if (!agent.is_empty() || !wkl.is_empty()) && !seen.insert((agent.to_owned(), wkl.to_owned())) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_detection_finds_duplicate_agent_wkl() {
        let chain = vec![
            ActChainEntry {
                sub: "a".into(),
                agent_id: Some("acme/agent".into()),
                wkl: Some("spiffe://acme/sa/a".into()),
                iat: 1,
            },
            ActChainEntry {
                sub: "b".into(),
                agent_id: Some("acme/agent".into()),
                wkl: Some("spiffe://acme/sa/a".into()),
                iat: 2,
            },
        ];
        assert!(act_chain_has_loop(&chain));
    }
}
