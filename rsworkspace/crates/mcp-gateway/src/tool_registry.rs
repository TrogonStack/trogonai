use crate::{KnownTool, ServerName, ServerToolSnapshot, ToolFingerprint, ToolName};
use std::collections::HashMap;

/// In-memory registry of previously observed tools, tool fingerprints, and
/// per-server manifest baselines.
///
/// The scanner library (`crate::scan`) is intentionally pure and owns no
/// state; per its README, "a caller... supplies comparison state". This is
/// that caller-supplied state, kept in process memory. It is not persisted
/// across restarts: on cold start every server's first `tools/list`
/// response is treated as a new baseline (matches AGT's `DriftDetector`
/// behavior for a server with no prior snapshot).
#[derive(Debug, Default)]
pub struct ToolRegistry {
    known_tools: Vec<KnownTool>,
    fingerprints: HashMap<(ServerName, ToolName), ToolFingerprint>,
    baselines: HashMap<ServerName, ServerToolSnapshot>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// All `(tool_name, server_name)` pairs seen so far, for typosquat and
    /// cross-server-impersonation detection.
    pub fn known_tools(&self) -> &[KnownTool] {
        &self.known_tools
    }

    /// Known tools belonging to servers other than `server_name`, which is
    /// what cross-server impersonation/typosquat checks must compare
    /// against (a tool cannot typosquat itself).
    pub fn known_tools_excluding_server(&self, server_name: &ServerName) -> Vec<KnownTool> {
        self.known_tools
            .iter()
            .filter(|known| known.server_name() != server_name)
            .cloned()
            .collect()
    }

    pub fn fingerprint(&self, server_name: &ServerName, tool_name: &ToolName) -> Option<&ToolFingerprint> {
        self.fingerprints.get(&(server_name.clone(), tool_name.clone()))
    }

    pub fn baseline(&self, server_name: &ServerName) -> Option<&ServerToolSnapshot> {
        self.baselines.get(server_name)
    }

    /// Record a tool as known, if not already present.
    pub fn remember_known_tool(&mut self, known: KnownTool) {
        if !self.known_tools.contains(&known) {
            self.known_tools.push(known);
        }
    }

    /// Insert or update a tool's fingerprint.
    pub fn upsert_fingerprint(&mut self, fingerprint: ToolFingerprint) {
        let key = (fingerprint.server_name().clone(), fingerprint.tool_name().clone());
        self.fingerprints.insert(key, fingerprint);
    }

    /// Replace a server's baseline manifest snapshot (used after drift
    /// comparison to adopt the current state as the new baseline).
    pub fn set_baseline(&mut self, snapshot: ServerToolSnapshot) {
        self.baselines.insert(snapshot.server_name().clone(), snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolDescription, ToolSchemaSnapshot};

    fn server(name: &str) -> ServerName {
        ServerName::new(name).expect("valid")
    }

    fn tool(name: &str) -> ToolName {
        ToolName::new(name).expect("valid")
    }

    #[test]
    fn remember_known_tool_deduplicates() {
        let mut registry = ToolRegistry::new();
        registry.remember_known_tool(KnownTool::new(tool("search"), server("web-tools")));
        registry.remember_known_tool(KnownTool::new(tool("search"), server("web-tools")));
        assert_eq!(registry.known_tools().len(), 1);
    }

    #[test]
    fn known_tools_excluding_server_filters_own_server() {
        let mut registry = ToolRegistry::new();
        registry.remember_known_tool(KnownTool::new(tool("search"), server("web-tools")));
        registry.remember_known_tool(KnownTool::new(tool("fetch"), server("other-tools")));
        let others = registry.known_tools_excluding_server(&server("web-tools"));
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].server_name().as_str(), "other-tools");
    }

    #[test]
    fn fingerprint_round_trips_by_server_and_tool() {
        let mut registry = ToolRegistry::new();
        let fp = ToolFingerprint::observe(
            tool("search"),
            server("web-tools"),
            &ToolDescription::new("Search the web"),
            None,
            1000.0,
        );
        registry.upsert_fingerprint(fp.clone());
        let found = registry
            .fingerprint(&server("web-tools"), &tool("search"))
            .expect("present");
        assert_eq!(found, &fp);
    }

    #[test]
    fn baseline_round_trips_by_server() {
        let mut registry = ToolRegistry::new();
        let snapshot = ServerToolSnapshot::new(
            server("web-tools"),
            vec![ToolSchemaSnapshot::new(
                tool("search"),
                "description",
                Default::default(),
                Vec::new(),
            )],
            1000.0,
        );
        registry.set_baseline(snapshot.clone());
        assert_eq!(registry.baseline(&server("web-tools")), Some(&snapshot));
    }

    #[test]
    fn missing_fingerprint_and_baseline_return_none() {
        let registry = ToolRegistry::new();
        assert!(registry.fingerprint(&server("web-tools"), &tool("search")).is_none());
        assert!(registry.baseline(&server("web-tools")).is_none());
    }
}
