use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

const ENV_MCP_GATEWAY_QUEUE_GROUP: &str = "MCP_GATEWAY_QUEUE_GROUP";
const ENV_MCP_GATEWAY_AUDIT_STREAM: &str = "MCP_GATEWAY_AUDIT_STREAM";
const ENV_MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT: &str = "MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT";
const ENV_MCP_GATEWAY_SPICEDB_ENDPOINT: &str = "MCP_GATEWAY_SPICEDB_ENDPOINT";
const ENV_MCP_GATEWAY_SPICEDB_INSECURE: &str = "MCP_GATEWAY_SPICEDB_INSECURE";
const ENV_MCP_GATEWAY_SPICEDB_TOKEN: &str = "MCP_GATEWAY_SPICEDB_TOKEN";

const ENV_MCP_PREFIX: &str = "MCP_PREFIX";
const ENV_NATS_URL: &str = "NATS_URL";
const ENV_NATS_CREDS: &str = "NATS_CREDS";
const DEFAULT_MCP_PREFIX: &str = "mcp";
const DEFAULT_NATS_URL: &str = "localhost:4222";
const DEFAULT_QUEUE_GROUP: &str = "mcp-gateway";
const DEFAULT_AUDIT_STREAM: &str = "MCP_AUDIT";

#[derive(Debug, Clone)]
pub struct CtlSettings {
    pub nats_url: String,
    pub nats_creds: Option<PathBuf>,
    pub mcp_prefix: String,
    pub queue_group: String,
    pub audit_stream_name: String,
    pub init_audit_stream: bool,
    pub spicedb_endpoint: Option<String>,
    pub spicedb_insecure: bool,
    pub spicedb_token_set: bool,
}

#[derive(Debug, Serialize)]
pub struct ConfigShowView {
    pub nats_url: String,
    pub nats_creds: Option<String>,
    pub mcp_prefix: String,
    pub queue_group: String,
    pub audit_stream_name: String,
    pub init_audit_stream: bool,
    pub audit_subject_wildcard: String,
    pub spicedb: SpicedbView,
}

#[derive(Debug, Serialize)]
pub struct SpicedbView {
    pub configured: bool,
    pub endpoint: Option<String>,
    pub insecure: bool,
    pub token_set: bool,
}

impl CtlSettings {
    pub fn from_env_and_config(config_path: Option<&Path>) -> Result<Self, String> {
        let mut overlay = HashMap::new();
        if let Some(path) = config_path {
            overlay = load_config_overlay(path)?;
        }

        let nats_url = overlay
            .get("nats_url")
            .cloned()
            .or_else(|| env::var(ENV_NATS_URL).ok())
            .unwrap_or_else(|| DEFAULT_NATS_URL.to_string());

        let nats_creds = overlay
            .get("nats_creds")
            .map(PathBuf::from)
            .or_else(|| env::var(ENV_NATS_CREDS).ok().map(PathBuf::from));

        let mcp_prefix = overlay
            .get("mcp_prefix")
            .cloned()
            .or_else(|| env::var(ENV_MCP_PREFIX).ok())
            .unwrap_or_else(|| DEFAULT_MCP_PREFIX.to_string());

        let queue_group = overlay
            .get("queue_group")
            .cloned()
            .or_else(|| env::var(ENV_MCP_GATEWAY_QUEUE_GROUP).ok())
            .unwrap_or_else(|| DEFAULT_QUEUE_GROUP.to_string());

        let audit_stream_name = overlay
            .get("audit_stream_name")
            .cloned()
            .or_else(|| env::var(ENV_MCP_GATEWAY_AUDIT_STREAM).ok())
            .unwrap_or_else(|| DEFAULT_AUDIT_STREAM.to_string());

        let init_audit_stream = !truthy_env(ENV_MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT)
            && !overlay
                .get("skip_audit_stream_init")
                .is_some_and(|value| is_truthy(value));

        let spicedb_endpoint = overlay
            .get("spicedb_endpoint")
            .cloned()
            .or_else(|| env::var(ENV_MCP_GATEWAY_SPICEDB_ENDPOINT).ok())
            .filter(|value| !value.trim().is_empty());

        let spicedb_insecure = overlay
            .get("spicedb_insecure")
            .is_some_and(|value| is_truthy(value))
            || truthy_env(ENV_MCP_GATEWAY_SPICEDB_INSECURE);

        let spicedb_token_set = overlay
            .get("spicedb_token")
            .is_some_and(|value| !value.trim().is_empty())
            || env::var(ENV_MCP_GATEWAY_SPICEDB_TOKEN)
                .ok()
                .is_some_and(|value| !value.trim().is_empty());

        Ok(Self {
            nats_url,
            nats_creds,
            mcp_prefix,
            queue_group,
            audit_stream_name,
            init_audit_stream,
            spicedb_endpoint,
            spicedb_insecure,
            spicedb_token_set,
        })
    }

    pub fn audit_subject_wildcard(&self) -> String {
        format!("{}.audit.>", self.mcp_prefix)
    }

    pub fn to_show_view(&self) -> ConfigShowView {
        ConfigShowView {
            nats_url: self.nats_url.clone(),
            nats_creds: self.nats_creds.as_ref().map(|path| path.display().to_string()),
            mcp_prefix: self.mcp_prefix.clone(),
            queue_group: self.queue_group.clone(),
            audit_stream_name: self.audit_stream_name.clone(),
            init_audit_stream: self.init_audit_stream,
            audit_subject_wildcard: self.audit_subject_wildcard(),
            spicedb: SpicedbView {
                configured: self.spicedb_endpoint.is_some(),
                endpoint: self.spicedb_endpoint.clone(),
                insecure: self.spicedb_insecure,
                token_set: self.spicedb_token_set,
            },
        }
    }
}

fn load_config_overlay(path: &Path) -> Result<HashMap<String, String>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("read config {}: {error}", path.display()))?;
    let table: toml::Table = toml::from_str(&raw)
        .map_err(|error| format!("parse config {}: {error}", path.display()))?;

    let mut overlay = HashMap::new();
    for (key, value) in table {
        if let Some(text) = value.as_str() {
            overlay.insert(key, text.to_string());
        } else if value.is_bool() {
            overlay.insert(key, value.to_string());
        }
    }
    Ok(overlay)
}

fn truthy_env(key: &str) -> bool {
    env::var(key)
        .ok()
        .is_some_and(|value| is_truthy(&value))
}

fn is_truthy(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_env_empty() {
        let settings = CtlSettings::from_env_and_config(None).expect("defaults");
        assert_eq!(settings.mcp_prefix, "mcp");
        assert_eq!(settings.audit_stream_name, "MCP_AUDIT");
        assert_eq!(settings.audit_subject_wildcard(), "mcp.audit.>");
    }
}
