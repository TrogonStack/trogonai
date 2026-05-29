use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use super::ruleset::RedactionRuleset;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RedactionDirection {
    Request,
    Response,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RegistryKey {
    server_id: String,
    tool_name: String,
    direction: RedactionDirection,
}

/// In-memory redaction rules keyed by server, tool, and direction.
/// Populated by bundle load in a future pass; tests and operators may register rules directly.
pub struct RedactionRegistry {
    rules: RwLock<HashMap<RegistryKey, RedactionRuleset>>,
}

impl RedactionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            rules: RwLock::new(HashMap::new()),
        }
    }

    pub fn install(registry: std::sync::Arc<Self>) -> Result<(), std::sync::Arc<Self>> {
        SHARED.set(registry)
    }

    #[must_use]
    pub fn shared() -> Option<std::sync::Arc<Self>> {
        SHARED.get().cloned()
    }

    pub fn register(
        &self,
        server_id: &str,
        tool_name: &str,
        direction: RedactionDirection,
        ruleset: RedactionRuleset,
    ) {
        let key = RegistryKey {
            server_id: server_id.to_string(),
            tool_name: tool_name.to_string(),
            direction,
        };
        self.rules
            .write()
            .expect("redaction registry lock")
            .insert(key, ruleset);
    }

    #[must_use]
    pub fn lookup(&self, server_id: &str, tool_name: &str, direction: RedactionDirection) -> Option<RedactionRuleset> {
        let key = RegistryKey {
            server_id: server_id.to_string(),
            tool_name: tool_name.to_string(),
            direction,
        };
        self.rules.read().expect("redaction registry lock").get(&key).cloned()
    }

    pub fn schema_tool_name(tool_name: &str, direction: RedactionDirection) -> String {
        match direction {
            RedactionDirection::Request => tool_name.to_string(),
            RedactionDirection::Response => format!("{tool_name}:output"),
        }
    }
}

impl Default for RedactionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

static SHARED: OnceLock<std::sync::Arc<RedactionRegistry>> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redaction::rule::{JsonPath, RedactionAction, RedactionRule};

    #[test]
    fn register_and_lookup_request_rules() {
        let registry = RedactionRegistry::new();
        let ruleset = RedactionRuleset::builder()
            .rule(RedactionRule {
                path: JsonPath::parse("$.params.token").expect("path"),
                action: RedactionAction::Hash,
            })
            .build();
        registry.register("github", "create_issue", RedactionDirection::Request, ruleset.clone());
        assert_eq!(
            registry.lookup("github", "create_issue", RedactionDirection::Request),
            Some(ruleset)
        );
        assert!(registry
            .lookup("github", "create_issue", RedactionDirection::Response)
            .is_none());
    }
}
