//! The constants of the settings document read path, taken from the pinned TypeScript bundle.
//!
//! `upstream/settings_schema.json` records them under `read`; the generator derives each list from the
//! bundle, so the drift check covers them and nothing here is transcribed by hand.

use serde_json::Value;

pub(crate) struct ReadSpec {
    /// Keys only honored from managed settings; a normal file loses them (`K7e`).
    pub(crate) managed_only: Vec<String>,
    /// `(alias, canonical)` pairs (`Iu`).
    pub(crate) aliases: Vec<(String, String)>,
    /// Keys whose `null` means "unset" (`GS()`).
    pub(crate) null_cleared: Vec<String>,
    /// Keys that accept a single server name where a list is expected (`D7e`).
    pub(crate) server_lists: Vec<String>,
    pub(crate) server_name: regex::Regex,
    pub(crate) hook_events: Vec<String>,
    /// Events whose hooks gate a tool call, so a bad entry may never be dropped silently (`Vf`).
    pub(crate) guard_events: Vec<String>,
    /// Top-level keys the stray-hook scan does not descend into (`b6`).
    pub(crate) unscanned: Vec<String>,
    pub(crate) hook_types: Vec<String>,
    pub(crate) cross_session_inbound: Vec<String>,
    pub(crate) share_host_profile: Vec<String>,
    pub(crate) marketplace_sources: Vec<String>,
    pub(crate) plugin_sources: Vec<String>,
    /// `(key, root schema name)` for the lists filtered entry by entry (`m7e`).
    pub(crate) mcp_lists: Vec<(String, String)>,
}

fn malformed(what: &str) -> String {
    format!("read.{what}")
}

fn strings(read: &Value, key: &str) -> Result<Vec<String>, String> {
    read.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| malformed(key))?
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or_else(|| malformed(key)))
        .collect()
}

fn pairs(read: &Value, key: &str, left: &str, right: &str) -> Result<Vec<(String, String)>, String> {
    read.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| malformed(key))?
        .iter()
        .map(|item| {
            let side = |name: &str| item.get(name).and_then(Value::as_str).map(str::to_owned);
            side(left).zip(side(right)).ok_or_else(|| malformed(key))
        })
        .collect()
}

impl ReadSpec {
    pub(crate) fn from_json(read: &Value) -> Result<Self, String> {
        let pattern = read
            .get("serverListPattern")
            .ok_or_else(|| malformed("serverListPattern"))?;
        if pattern.get("flags").and_then(Value::as_str) != Some("") {
            return Err(malformed("serverListPattern flags"));
        }
        let source = pattern
            .get("regex")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("serverListPattern"))?;
        Ok(Self {
            managed_only: strings(read, "managedOnlyKeys")?,
            aliases: pairs(read, "aliases", "alias", "canonical")?,
            null_cleared: strings(read, "nullClearedKeys")?,
            server_lists: strings(read, "serverListKeys")?,
            server_name: regex::Regex::new(source).map_err(|error| error.to_string())?,
            hook_events: strings(read, "hookEvents")?,
            guard_events: strings(read, "guardEvents")?,
            unscanned: strings(read, "unscannedKeys")?,
            hook_types: strings(read, "hookTypes")?,
            cross_session_inbound: strings(read, "crossSessionInbound")?,
            share_host_profile: strings(read, "shareHostProfile")?,
            marketplace_sources: strings(read, "knownMarketplaceSources")?,
            plugin_sources: strings(read, "knownPluginSources")?,
            mcp_lists: pairs(read, "mcpLists", "key", "root")?,
        })
    }
}
