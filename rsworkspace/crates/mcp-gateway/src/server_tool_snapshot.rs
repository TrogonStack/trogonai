use crate::{ServerName, ToolName, ToolSchemaSnapshot};

/// Point-in-time record of an MCP server's full tool manifest, used as
/// either a drift baseline or the current state to compare against one.
/// Matches AGT's `ToolSnapshot` dataclass.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerToolSnapshot {
    server_name: ServerName,
    tools: Vec<ToolSchemaSnapshot>,
    captured_at_epoch_seconds: f64,
}

impl ServerToolSnapshot {
    pub fn new(server_name: ServerName, tools: Vec<ToolSchemaSnapshot>, captured_at_epoch_seconds: f64) -> Self {
        Self {
            server_name,
            tools,
            captured_at_epoch_seconds,
        }
    }

    pub fn server_name(&self) -> &ServerName {
        &self.server_name
    }

    pub fn tools(&self) -> &[ToolSchemaSnapshot] {
        &self.tools
    }

    pub fn captured_at_epoch_seconds(&self) -> f64 {
        self.captured_at_epoch_seconds
    }

    pub fn tool_names(&self) -> Vec<&ToolName> {
        self.tools.iter().map(ToolSchemaSnapshot::name).collect()
    }

    pub fn get_tool(&self, name: &ToolName) -> Option<&ToolSchemaSnapshot> {
        self.tools.iter().find(|t| t.name() == name)
    }

    /// Combined fingerprint of every tool's fingerprint, sorted for order
    /// independence. Matches AGT's
    /// `hashlib.sha256("|".join(sorted(fingerprints))).hexdigest()[:16]`.
    pub fn fingerprint(&self) -> String {
        let mut parts: Vec<String> = self.tools.iter().map(ToolSchemaSnapshot::fingerprint).collect();
        parts.sort();
        let joined = parts.join("|");
        let digest = crate::Sha256Digest::of(joined.as_bytes());
        digest.as_str().chars().take(16).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> ToolSchemaSnapshot {
        ToolSchemaSnapshot::new(
            ToolName::new(name).expect("valid"),
            "description",
            Default::default(),
            Vec::new(),
        )
    }

    fn server() -> ServerName {
        ServerName::new("web-tools").expect("valid")
    }

    #[test]
    fn tool_names_lists_all_tools() {
        let snapshot = ServerToolSnapshot::new(server(), vec![tool("a"), tool("b")], 1000.0);
        let names: Vec<&str> = snapshot.tool_names().iter().map(|n| n.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn get_tool_finds_by_name() {
        let snapshot = ServerToolSnapshot::new(server(), vec![tool("a")], 1000.0);
        assert!(snapshot.get_tool(&ToolName::new("a").expect("valid")).is_some());
        assert!(snapshot.get_tool(&ToolName::new("missing").expect("valid")).is_none());
    }

    #[test]
    fn fingerprint_is_order_independent() {
        let a = ServerToolSnapshot::new(server(), vec![tool("a"), tool("b")], 1000.0);
        let b = ServerToolSnapshot::new(server(), vec![tool("b"), tool("a")], 1000.0);
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_changes_when_tool_set_changes() {
        let a = ServerToolSnapshot::new(server(), vec![tool("a")], 1000.0);
        let b = ServerToolSnapshot::new(server(), vec![tool("a"), tool("b")], 1000.0);
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
