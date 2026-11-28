//! Which MCP catalog one model step was offered.

use serde::{Deserialize, Serialize};

use crate::ContentHash;

/// The MCP catalog one model step was offered, and who vouched for it.
///
/// Not to be confused with `trogonai_harness_mcp`'s input identity witness, which records the
/// epochs a single in-flight request was issued against so a late response can
/// be dropped. This records what the model was *told*, which is why
/// [`Self::same_definition`] deliberately ignores most of the epochs: a
/// transport that reconnects has not changed the action it advertised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct McpToolBinding {
    /// Bumped whenever the runtime itself is replaced.
    pub runtime_generation: u64,
    /// Bumped whenever the connection is replaced.
    pub connection_generation: u64,
    /// Bumped whenever the tool catalog is replaced.
    pub catalog_generation: u64,
    /// Bumped whenever the credentials are replaced.
    pub auth_generation: u64,
    /// Which authority approved the catalog.
    pub authority_id: u64,
    /// A digest of the advertised definition, over the tool and the server's
    /// instructions, so a reworded instruction is a new advertisement.
    pub definition_digest: ContentHash,
}

impl McpToolBinding {
    /// Whether both bindings advertise the same action under the same authority.
    ///
    /// The connection, catalog and auth epochs move for reasons the model never
    /// saw: a reconnect, a re-listing that returned the same tools, a refreshed
    /// token. Comparing them would retract an advertisement that still holds.
    #[must_use]
    pub fn same_definition(self, other: Self) -> bool {
        self.runtime_generation == other.runtime_generation
            && self.authority_id == other.authority_id
            && self.definition_digest == other.definition_digest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> McpToolBinding {
        McpToolBinding {
            runtime_generation: 3,
            connection_generation: 7,
            catalog_generation: 11,
            auth_generation: 2,
            authority_id: 41,
            definition_digest: ContentHash::of(b"search(query)"),
        }
    }

    #[test]
    fn a_reconnect_that_readvertises_the_same_action_is_still_the_same_binding() {
        let renewed = McpToolBinding {
            connection_generation: 8,
            catalog_generation: 12,
            auth_generation: 3,
            ..binding()
        };

        assert!(binding().same_definition(renewed));
    }

    #[test]
    fn a_changed_action_authority_or_runtime_is_a_different_binding() {
        let redefined = McpToolBinding {
            definition_digest: ContentHash::of(b"search(query, limit)"),
            ..binding()
        };
        let reauthorized = McpToolBinding {
            authority_id: 42,
            ..binding()
        };
        let restarted = McpToolBinding {
            runtime_generation: 4,
            ..binding()
        };

        assert!(!binding().same_definition(redefined));
        assert!(!binding().same_definition(reauthorized));
        assert!(!binding().same_definition(restarted));
    }
}
