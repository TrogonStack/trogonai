use crate::{ServerName, Sha256Digest, ToolDescription, ToolInputSchema, ToolName};

/// Cryptographic fingerprint of a tool definition, used to detect rug pulls
/// (silent changes to a tool's description or schema between registrations).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolFingerprint {
    tool_name: ToolName,
    server_name: ServerName,
    description_hash: Sha256Digest,
    schema_hash: Sha256Digest,
    first_seen_epoch_seconds: f64,
    last_seen_epoch_seconds: f64,
    version: u32,
}

impl ToolFingerprint {
    /// Compute the description and schema hashes for a tool definition and
    /// wrap them as a brand-new, version-1 fingerprint observed at `now`.
    pub fn observe(
        tool_name: ToolName,
        server_name: ServerName,
        description: &ToolDescription,
        schema: Option<&ToolInputSchema>,
        now_epoch_seconds: f64,
    ) -> Self {
        Self {
            tool_name,
            server_name,
            description_hash: hash_description(description),
            schema_hash: hash_schema(schema),
            first_seen_epoch_seconds: now_epoch_seconds,
            last_seen_epoch_seconds: now_epoch_seconds,
            version: 1,
        }
    }

    /// Re-observe the same tool. Bumps the version and hashes only when the
    /// description or schema actually changed; otherwise just refreshes
    /// `last_seen_epoch_seconds`. Mirrors AGT's `register_tool()`.
    pub fn reobserve(
        &mut self,
        description: &ToolDescription,
        schema: Option<&ToolInputSchema>,
        now_epoch_seconds: f64,
    ) {
        let description_hash = hash_description(description);
        let schema_hash = hash_schema(schema);
        if description_hash != self.description_hash || schema_hash != self.schema_hash {
            self.description_hash = description_hash;
            self.schema_hash = schema_hash;
            self.version += 1;
        }
        self.last_seen_epoch_seconds = now_epoch_seconds;
    }

    pub fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    pub fn server_name(&self) -> &ServerName {
        &self.server_name
    }

    pub fn description_hash(&self) -> &Sha256Digest {
        &self.description_hash
    }

    pub fn schema_hash(&self) -> &Sha256Digest {
        &self.schema_hash
    }

    pub fn first_seen_epoch_seconds(&self) -> f64 {
        self.first_seen_epoch_seconds
    }

    pub fn last_seen_epoch_seconds(&self) -> f64 {
        self.last_seen_epoch_seconds
    }

    pub fn version(&self) -> u32 {
        self.version
    }
}

/// SHA-256 over the UTF-8 bytes of the description text, matching AGT's
/// `hashlib.sha256(description.encode("utf-8")).hexdigest()`.
pub(crate) fn hash_description(description: &ToolDescription) -> Sha256Digest {
    Sha256Digest::of(description.as_str().as_bytes())
}

/// SHA-256 over the schema's canonical JSON serialization, or over an empty
/// byte string when no schema is present. Matches AGT's
/// `hashlib.sha256(json.dumps(schema, sort_keys=True, default=str) ... )`.
pub(crate) fn hash_schema(schema: Option<&ToolInputSchema>) -> Sha256Digest {
    match schema {
        Some(schema) => Sha256Digest::of(schema.canonical_json().as_bytes()),
        None => Sha256Digest::of(b""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_name() -> ToolName {
        ToolName::new("search").expect("valid tool name")
    }

    fn server_name() -> ServerName {
        ServerName::new("web-tools").expect("valid server name")
    }

    #[test]
    fn observe_starts_at_version_one() {
        let fp = ToolFingerprint::observe(
            tool_name(),
            server_name(),
            &ToolDescription::new("Search the web"),
            None,
            1000.0,
        );
        assert_eq!(fp.version(), 1);
        assert_eq!(fp.first_seen_epoch_seconds(), 1000.0);
        assert_eq!(fp.last_seen_epoch_seconds(), 1000.0);
    }

    #[test]
    fn description_hash_is_sha256_of_utf8_bytes() {
        let fp = ToolFingerprint::observe(
            tool_name(),
            server_name(),
            &ToolDescription::new("Search the web"),
            None,
            1000.0,
        );
        assert_eq!(fp.description_hash(), &Sha256Digest::of("Search the web".as_bytes()));
    }

    #[test]
    fn reobserve_bumps_version_on_description_change() {
        let mut fp = ToolFingerprint::observe(
            tool_name(),
            server_name(),
            &ToolDescription::new("Search v1"),
            None,
            1000.0,
        );
        fp.reobserve(&ToolDescription::new("Search v2"), None, 2000.0);
        assert_eq!(fp.version(), 2);
        assert_eq!(fp.last_seen_epoch_seconds(), 2000.0);
    }

    #[test]
    fn reobserve_keeps_version_when_unchanged() {
        let mut fp = ToolFingerprint::observe(
            tool_name(),
            server_name(),
            &ToolDescription::new("Search the web"),
            None,
            1000.0,
        );
        fp.reobserve(&ToolDescription::new("Search the web"), None, 2000.0);
        assert_eq!(fp.version(), 1);
        assert_eq!(fp.last_seen_epoch_seconds(), 2000.0);
    }

    #[test]
    fn reobserve_bumps_version_on_schema_change() {
        let schema_v1 = ToolInputSchema::new(json!({"type": "object", "properties": {"q": {"type": "string"}}}))
            .expect("valid schema");
        let schema_v2 = ToolInputSchema::new(json!({
            "type": "object",
            "properties": {"q": {"type": "string"}, "exec": {"type": "string"}}
        }))
        .expect("valid schema");
        let mut fp = ToolFingerprint::observe(
            tool_name(),
            server_name(),
            &ToolDescription::new("Search the web"),
            Some(&schema_v1),
            1000.0,
        );
        fp.reobserve(&ToolDescription::new("Search the web"), Some(&schema_v2), 2000.0);
        assert_eq!(fp.version(), 2);
    }
}
