//! Scope — a declarative permission envelope for a low-friction agentic loop.
//!
//! The model: the agent runs freely **inside** a declared boundary (write roots,
//! runnable commands, network) with no prompts, and is interrupted **only** when
//! it tries to **cross** that boundary. A single [`Scope`] value captures both
//! what the agent may do and what happens at the edge ([`OnExceed`]) — approval
//! is the boundary's exception path, not a separate dial. This deliberately does
//! NOT copy Codex's two free axes (`sandbox_mode` x `approval_policy`).
//!
//! This module (SCOPE-1) defines the type vocabulary only. The decision logic
//! ([`Scope::evaluate`]), the hardcoded baseline, and wire-config loading land in
//! later tasks; nothing here is wired into the live permission path yet.
//!
//! Path and command matching reuse the crate's existing `globset` dependency and
//! mirror the semantics of `permission_rules` (`*`/`**` match-all short-circuit
//! for globs; lone `*` or exact/prefix match for commands).

use globset::{Glob, GlobSet as GlobSetInner, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::permission::{
    is_edit_tool, is_read_only_bash_command, is_read_only_tool, lexical_abs, touches_protected_path,
};
use crate::permission_rules::{extract_path_from_input, normalize_tool_name};

/// What happens when a tool call falls **outside** the scope.
///
/// This is the single knob that survives from a two-axis approval model: it is a
/// property of the boundary, not a global setting, and only ever applies on a
/// crossing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnExceed {
    /// Prompt the human once via the existing permission channel.
    Escalate,
    /// Fail closed without prompting (unattended / CI).
    Deny,
}

/// Network access permitted to network-capable tools within the scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    /// No network — `fetch_url`, `web_search`, `git_push`, `gh` are out of scope.
    Denied,
    /// Network allowed only to the listed hosts (layers over `EgressPolicy`).
    AllowList(Vec<String>),
    /// Unrestricted network.
    Allowed,
}

/// Outcome of evaluating a single tool call against a [`Scope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeDecision {
    /// Inside the envelope — auto-allow, no prompt, no round-trip.
    InScope,
    /// Outside the envelope — the caller applies the scope's [`OnExceed`].
    OutOfScope,
    /// Hit a hard boundary (`protected`) — deny, never prompt.
    Forbidden,
}

/// Error converting untrusted wire config into a domain [`Scope`].
#[derive(Debug)]
pub enum ScopeError {
    /// A glob pattern failed to compile.
    InvalidGlob {
        pattern: String,
        source: globset::Error,
    },
    /// The `network` field held an unrecognized value.
    UnknownNetwork(String),
    /// The `on_exceed` field held an unrecognized value.
    UnknownOnExceed(String),
}

impl std::fmt::Display for ScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScopeError::InvalidGlob { pattern, .. } => {
                write!(f, "invalid glob pattern `{pattern}`")
            }
            ScopeError::UnknownNetwork(value) => write!(
                f,
                "unknown network policy `{value}` (expected `off`, `on`, or a host list)"
            ),
            ScopeError::UnknownOnExceed(value) => {
                write!(f, "unknown on_exceed `{value}` (expected `escalate` or `deny`)")
            }
        }
    }
}

impl std::error::Error for ScopeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ScopeError::InvalidGlob { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A compiled set of path globs, validated at construction.
///
/// Wraps the crate's `globset` dependency and keeps the source patterns for
/// `Debug`/equality. Mirrors `permission_rules`: an empty set matches nothing,
/// and a `*` or `**` pattern matches everything.
#[derive(Clone)]
pub struct GlobSet {
    sources: Vec<String>,
    set: GlobSetInner,
    match_all: bool,
}

impl GlobSet {
    /// An empty set that matches no path.
    pub fn empty() -> Self {
        Self {
            sources: Vec::new(),
            set: GlobSetInner::empty(),
            match_all: false,
        }
    }

    /// Compile a set of glob patterns, surfacing the first invalid one.
    pub fn compile(patterns: &[String]) -> Result<Self, ScopeError> {
        let match_all = patterns.iter().any(|p| p == "*" || p == "**");
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            let glob = Glob::new(pattern).map_err(|source| ScopeError::InvalidGlob {
                pattern: pattern.clone(),
                source,
            })?;
            builder.add(glob);
        }
        let set = builder.build().map_err(|source| ScopeError::InvalidGlob {
            pattern: patterns.join(", "),
            source,
        })?;
        Ok(Self {
            sources: patterns.to_vec(),
            set,
            match_all,
        })
    }

    /// Whether `path` is matched by any pattern in the set.
    pub fn matches(&self, path: &str) -> bool {
        self.match_all || self.set.is_match(path)
    }

    /// The source patterns this set was built from.
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// Whether the set holds no patterns.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

impl std::fmt::Debug for GlobSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlobSet")
            .field("sources", &self.sources)
            .field("match_all", &self.match_all)
            .finish()
    }
}

// `globset::GlobSet` is not `PartialEq`; equality is defined by the source
// patterns and the match-all short-circuit, which fully determine behavior.
impl PartialEq for GlobSet {
    fn eq(&self, other: &Self) -> bool {
        self.match_all == other.match_all && self.sources == other.sources
    }
}

impl Eq for GlobSet {}

/// A set of command-prefix patterns runnable within the scope.
///
/// Mirrors `permission_rules::matches_any_prefix`: a lone `*` matches any
/// command; otherwise a pattern matches a command equal to it or beginning with
/// it followed by whitespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSet {
    patterns: Vec<String>,
}

impl CommandSet {
    /// A set that permits any command.
    pub fn any() -> Self {
        Self {
            patterns: vec!["*".to_string()],
        }
    }

    /// A set that permits no command.
    pub fn empty() -> Self {
        Self {
            patterns: Vec::new(),
        }
    }

    /// Build a set from explicit prefix patterns.
    pub fn from_patterns(patterns: Vec<String>) -> Self {
        Self { patterns }
    }

    /// Whether `command` matches any prefix pattern in the set.
    pub fn matches(&self, command: &str) -> bool {
        let trimmed = command.trim();
        self.patterns.iter().any(|pattern| {
            if pattern == "*" {
                return true;
            }
            trimmed == pattern.as_str()
                || trimmed.starts_with(&format!("{pattern} "))
                || trimmed.starts_with(&format!("{pattern}\n"))
        })
    }

    /// The source patterns this set was built from.
    pub fn sources(&self) -> &[String] {
        &self.patterns
    }

    /// Whether the set holds no patterns.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// A declarative permission envelope for one session.
///
/// Inside the envelope the agent runs silently; crossing it triggers
/// [`OnExceed`]; touching `protected` is a hard deny. Constructed via the
/// `baseline` / `from_wire` builders — fields are private so an invalid `Scope`
/// is unrepresentable.
///
/// Serde persistence routes through [`ScopePersist`] (the source patterns, which
/// are already cwd-anchored at construction), so a `Scope` round-trips without
/// needing the cwd again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ScopePersist", into = "ScopePersist")]
pub struct Scope {
    write: GlobSet,
    run: CommandSet,
    network: NetworkPolicy,
    protected: GlobSet,
    on_exceed: OnExceed,
}

/// Serde-friendly form of a [`Scope`]: the raw (already-anchored) source patterns
/// plus the enum fields. Rebuilding recompiles the globs without re-anchoring.
#[derive(Serialize, Deserialize)]
struct ScopePersist {
    #[serde(default)]
    write: Vec<String>,
    #[serde(default)]
    run: Vec<String>,
    network: NetworkPolicy,
    #[serde(default)]
    protected: Vec<String>,
    on_exceed: OnExceed,
}

impl From<Scope> for ScopePersist {
    fn from(scope: Scope) -> Self {
        Self {
            write: scope.write.sources().to_vec(),
            run: scope.run.sources().to_vec(),
            network: scope.network,
            protected: scope.protected.sources().to_vec(),
            on_exceed: scope.on_exceed,
        }
    }
}

impl TryFrom<ScopePersist> for Scope {
    type Error = ScopeError;

    fn try_from(persist: ScopePersist) -> Result<Self, Self::Error> {
        Ok(Self {
            write: GlobSet::compile(&persist.write)?,
            run: CommandSet::from_patterns(persist.run),
            network: persist.network,
            protected: GlobSet::compile(&persist.protected)?,
            on_exceed: persist.on_exceed,
        })
    }
}

impl Scope {
    /// The hardcoded, compiled-in default scope: the agent may write anywhere
    /// **within the current working directory** and run any command silently,
    /// network is denied, and anything outside the boundary escalates once.
    ///
    /// The write root is cwd-anchored (`{cwd}/**`) rather than a bare `**`, so a
    /// write resolved outside the working directory falls *out of scope* and
    /// escalates instead of being silently allowed. `cwd` is escaped so glob
    /// metacharacters in the path are treated literally. Pure; performs no I/O.
    pub fn baseline(cwd: &str) -> Self {
        let write = GlobSet::compile(&anchor_to_cwd(&["**".to_string()], cwd))
            .expect("cwd-anchored `**` is a valid glob");
        Self {
            write,
            run: CommandSet::any(),
            network: NetworkPolicy::Denied,
            protected: GlobSet::empty(),
            on_exceed: OnExceed::Escalate,
        }
    }

    /// Convert untrusted wire config into a validated [`Scope`], anchoring
    /// relative globs to `cwd`. This is the single wire -> domain boundary; it
    /// runs exactly once per config source. Returns the first [`ScopeError`]
    /// encountered (invalid glob, or unrecognized `network` / `on_exceed`).
    ///
    /// `run` is taken verbatim — an empty list means "no commands" (bash blocked),
    /// not "any command"; the run-any default lives only in [`Scope::baseline`].
    pub fn from_wire(wire: ScopeWire, cwd: &str) -> Result<Self, ScopeError> {
        let write = GlobSet::compile(&anchor_to_cwd(&wire.write, cwd))?;
        let protected = GlobSet::compile(&anchor_to_cwd(&wire.protected, cwd))?;
        Ok(Self {
            write,
            run: CommandSet::from_patterns(wire.run),
            network: parse_network(wire.network)?,
            protected,
            on_exceed: parse_on_exceed(wire.on_exceed)?,
        })
    }

    /// Classify a single tool call against this scope.
    ///
    /// Order of decision:
    /// 1. **Protected** — the global secrets/VCS-internals set, or this scope's
    ///    `protected` globs, are a hard boundary (`Forbidden`, never escalatable).
    /// 2. **Reads** — read-only tools and read-only bash are always `InScope`.
    /// 3. **Network tools** (`fetch_url`, `web_search`, `git_push`, `gh`) depend on
    ///    the network policy. `AllowList` is permitted at this gate; per-host
    ///    enforcement is delegated to `EgressPolicy` downstream.
    /// 4. **File writes/edits** — `InScope` only when the resolved target path
    ///    falls under a write root; a missing path fails closed.
    /// 5. **Local VCS writes** (`git_commit`, `git_create_branch`) — `InScope` only
    ///    when the working directory is itself within a write root.
    /// 6. **Write-bash** — `InScope` only when the command matches a `run` pattern.
    /// 7. **Anything else** — fails closed (`OutOfScope`).
    ///
    /// `OutOfScope` is not a denial: the caller applies [`Scope::on_exceed`]
    /// (escalate once, or deny). Only `Forbidden` is a hard stop.
    pub fn evaluate(&self, tool_name: &str, tool_input: &Value, cwd: &str) -> ScopeDecision {
        // 1. Hard boundary first.
        if touches_protected_path(tool_name, tool_input) {
            return ScopeDecision::Forbidden;
        }
        if let Some(path) = extract_path_from_input(tool_input) {
            let abs = lexical_abs(cwd, path);
            if self.protected.matches(&abs.to_string_lossy()) {
                return ScopeDecision::Forbidden;
            }
        }

        let normalized = normalize_tool_name(tool_name);

        // 2. Reads are always in scope.
        if is_read_only_tool(tool_name)
            || (normalized == "bash" && is_read_only_bash_command(tool_input))
        {
            return ScopeDecision::InScope;
        }

        // 3. Network-capable tools depend on the network policy.
        if matches!(normalized, "fetch_url" | "web_search" | "git_push" | "gh") {
            return if matches!(self.network, NetworkPolicy::Denied) {
                ScopeDecision::OutOfScope
            } else {
                ScopeDecision::InScope
            };
        }

        // 4. File-write / edit tools: in scope iff the resolved path is writable.
        if is_edit_tool(tool_name) || matches!(normalized, "multi_edit" | "delete_file") {
            let Some(path) = extract_path_from_input(tool_input) else {
                return ScopeDecision::OutOfScope; // no path → fail closed
            };
            let abs = lexical_abs(cwd, path);
            return if self.write.matches(&abs.to_string_lossy()) {
                ScopeDecision::InScope
            } else {
                ScopeDecision::OutOfScope
            };
        }

        // 5. Local VCS writes: in scope iff the working directory is writable.
        //    git writes occur under cwd, so probe a representative path under it.
        if matches!(normalized, "git_commit" | "git_create_branch") {
            let under_cwd = lexical_abs(cwd, "_scope_probe_");
            return if self.write.matches(&under_cwd.to_string_lossy()) {
                ScopeDecision::InScope
            } else {
                ScopeDecision::OutOfScope
            };
        }

        // 6. Write-bash: in scope iff the command matches an allowed run pattern.
        //    (Outright-dangerous commands are caught by deny rules upstream of
        //    scope, so run-any baselines stay safe in the live flow.)
        if normalized == "bash" {
            let cmd = tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            return if self.run.matches(cmd) {
                ScopeDecision::InScope
            } else {
                ScopeDecision::OutOfScope
            };
        }

        // 7. Unknown tool: fail closed.
        ScopeDecision::OutOfScope
    }

    /// Globs the agent may write to silently.
    pub fn write(&self) -> &GlobSet {
        &self.write
    }

    /// Command prefixes the agent may run silently.
    pub fn run(&self) -> &CommandSet {
        &self.run
    }

    /// Network access permitted within the scope.
    pub fn network(&self) -> &NetworkPolicy {
        &self.network
    }

    /// Additive hard-deny globs, on top of the global protected set.
    pub fn protected(&self) -> &GlobSet {
        &self.protected
    }

    /// Behavior when a tool call falls outside the scope.
    pub fn on_exceed(&self) -> OnExceed {
        self.on_exceed
    }

    /// Resolve the effective scope from all sources, in precedence order:
    /// session (already a built Scope) > settings.json > TROGON.md > compiled baseline.
    ///
    /// The first present source wins; config wires are built via [`Scope::from_wire`].
    pub fn resolve(
        session: Option<Scope>,
        settings: Option<ScopeWire>,
        trogon_md: Option<ScopeWire>,
        cwd: &str,
    ) -> Result<Self, ScopeError> {
        if let Some(session) = session {
            return Ok(session);
        }
        if let Some(settings) = settings {
            return Self::from_wire(settings, cwd);
        }
        if let Some(trogon_md) = trogon_md {
            return Self::from_wire(trogon_md, cwd);
        }
        Ok(Self::baseline(cwd))
    }
}

/// Untrusted wire form of a [`Scope`], deserialized from TROGON.md / settings.json
/// / NATS. Converted into a domain [`Scope`] exactly once via [`Scope::from_wire`].
///
/// Field grammars:
/// - `network`: `off` | `on` | `allow:host1,host2,...` (absent => denied)
/// - `on_exceed`: `escalate` | `deny` (absent => escalate)
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ScopeWire {
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub run: Vec<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub protected: Vec<String>,
    #[serde(default)]
    pub on_exceed: Option<String>,
}

fn parse_scope_list(values: &str) -> Vec<String> {
    values
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_scope_line(line: &str, wire: &mut ScopeWire) {
    let trimmed = line.trim();
    if trimmed.starts_with("```") {
        return;
    }

    let Some((key, values)) = trimmed.split_once(':') else {
        return;
    };

    match key.trim() {
        "scope.write" => wire.write.extend(parse_scope_list(values)),
        "scope.run" => wire.run.extend(parse_scope_list(values)),
        "scope.protected" => wire.protected.extend(parse_scope_list(values)),
        "scope.network" => {
            let val = values.trim();
            if !val.is_empty() {
                wire.network = Some(val.to_string());
            }
        }
        "scope.on_exceed" => {
            let val = values.trim();
            if !val.is_empty() {
                wire.on_exceed = Some(val.to_string());
            }
        }
        _ => {}
    }
}

impl ScopeWire {
    /// Parse scope configuration from TROGON.md text.
    ///
    /// Reads `scope.*:` bare lines, but **skips anything inside a fenced code
    /// block** (```` ``` ```` or `~~~`). A leading `[scope]` header line, if
    /// present, is ignored. Missing keys leave fields at their defaults.
    pub fn from_trogon_md(text: &str) -> Self {
        let mut wire = Self::default();
        let mut in_fence = false;
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                continue;
            }
            parse_scope_line(line, &mut wire);
        }
        wire
    }

    /// Extract scope config from a parsed settings.json value. Reads the top-level
    /// `"scope"` object (if present) and deserializes it as a [`ScopeWire`]. Absent or
    /// non-object => [`ScopeWire::default()`].
    pub fn from_settings_value(settings: &Value) -> Self {
        settings
            .get("scope")
            .filter(|scope| scope.is_object())
            .and_then(|scope| serde_json::from_value(scope.clone()).ok())
            .unwrap_or_default()
    }
}

/// Anchor relative glob patterns to `cwd` (escaped, so the path is literal) so
/// they match the absolute paths produced during evaluation. Absolute patterns
/// (leading `/`) are left untouched.
fn anchor_to_cwd(patterns: &[String], cwd: &str) -> Vec<String> {
    let root = globset::escape(cwd.trim_end_matches('/'));
    patterns
        .iter()
        .map(|pattern| {
            if pattern.starts_with('/') {
                pattern.clone()
            } else {
                format!("{root}/{pattern}")
            }
        })
        .collect()
}

/// Parse the wire `network` field. Absent => `Denied`.
fn parse_network(value: Option<String>) -> Result<NetworkPolicy, ScopeError> {
    let Some(raw) = value else {
        return Ok(NetworkPolicy::Denied);
    };
    match raw.trim() {
        "off" => Ok(NetworkPolicy::Denied),
        "on" => Ok(NetworkPolicy::Allowed),
        other => match other.strip_prefix("allow:") {
            Some(list) => {
                let hosts: Vec<String> = list
                    .split(',')
                    .map(|h| h.trim().to_string())
                    .filter(|h| !h.is_empty())
                    .collect();
                if hosts.is_empty() {
                    Err(ScopeError::UnknownNetwork(raw))
                } else {
                    Ok(NetworkPolicy::AllowList(hosts))
                }
            }
            None => Err(ScopeError::UnknownNetwork(raw)),
        },
    }
}

/// Parse the wire `on_exceed` field. Absent => `Escalate`.
fn parse_on_exceed(value: Option<String>) -> Result<OnExceed, ScopeError> {
    let Some(raw) = value else {
        return Ok(OnExceed::Escalate);
    };
    match raw.trim() {
        "escalate" => Ok(OnExceed::Escalate),
        "deny" => Ok(OnExceed::Deny),
        _ => Err(ScopeError::UnknownOnExceed(raw)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn baseline_anchors_writes_to_cwd_and_denies_network() {
        let scope = Scope::baseline("/repo");
        // Writes inside the working directory are in scope.
        assert!(scope.write().matches("/repo/src/main.rs"));
        assert!(scope.write().matches("/repo/Cargo.toml"));
        // Writes outside it are not (they will escalate, not auto-allow).
        assert!(!scope.write().matches("/etc/passwd"));
        assert!(!scope.write().matches("/repo-sibling/x"));
        // Any command runs; network is denied; out-of-scope escalates.
        assert!(scope.run().matches("cargo test --workspace"));
        assert_eq!(scope.network(), &NetworkPolicy::Denied);
        assert_eq!(scope.on_exceed(), OnExceed::Escalate);
        assert!(scope.protected().is_empty());
    }

    #[test]
    fn baseline_trims_trailing_slash_in_cwd() {
        let scope = Scope::baseline("/repo/");
        assert!(scope.write().matches("/repo/src/lib.rs"));
        assert!(!scope.write().matches("/other/file"));
    }

    #[test]
    fn from_wire_happy_path() {
        let wire = ScopeWire {
            write: vec!["src/**".to_string()],
            run: vec!["cargo".to_string()],
            network: Some("allow:api.github.com, example.com".to_string()),
            protected: vec!["**/.env".to_string()],
            on_exceed: Some("deny".to_string()),
        };
        let scope = Scope::from_wire(wire, "/repo").expect("valid wire");
        // Relative write glob anchored to cwd.
        assert!(scope.write().matches("/repo/src/main.rs"));
        assert!(!scope.write().matches("/repo/tests/x.rs"));
        // Command prefix matching.
        assert!(scope.run().matches("cargo build"));
        assert!(!scope.run().matches("rm -rf /"));
        // Host allow-list parsed and trimmed.
        assert_eq!(
            scope.network(),
            &NetworkPolicy::AllowList(vec![
                "api.github.com".to_string(),
                "example.com".to_string()
            ])
        );
        assert_eq!(scope.on_exceed(), OnExceed::Deny);
        assert!(scope.protected().matches("/repo/src/.env"));
    }

    #[test]
    fn from_wire_defaults_to_denied_and_escalate() {
        let scope = Scope::from_wire(ScopeWire::default(), "/repo").expect("valid");
        assert_eq!(scope.network(), &NetworkPolicy::Denied);
        assert_eq!(scope.on_exceed(), OnExceed::Escalate);
        assert!(scope.write().is_empty());
        // Empty `run` means no commands, not "any".
        assert!(scope.run().is_empty());
        assert!(!scope.run().matches("cargo build"));
    }

    #[test]
    fn from_wire_network_on_off() {
        let on = Scope::from_wire(
            ScopeWire {
                network: Some("on".to_string()),
                ..Default::default()
            },
            "/repo",
        )
        .unwrap();
        assert_eq!(on.network(), &NetworkPolicy::Allowed);
        let off = Scope::from_wire(
            ScopeWire {
                network: Some("off".to_string()),
                ..Default::default()
            },
            "/repo",
        )
        .unwrap();
        assert_eq!(off.network(), &NetworkPolicy::Denied);
    }

    #[test]
    fn from_wire_invalid_glob_errors() {
        let wire = ScopeWire {
            write: vec!["[".to_string()],
            ..Default::default()
        };
        let err = Scope::from_wire(wire, "/repo").unwrap_err();
        assert!(matches!(err, ScopeError::InvalidGlob { .. }));
        // Error implements std::error::Error with a source.
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn from_wire_unknown_network_errors() {
        for bad in ["sometimes", "allow:", "yes"] {
            let wire = ScopeWire {
                network: Some(bad.to_string()),
                ..Default::default()
            };
            assert!(matches!(
                Scope::from_wire(wire, "/repo").unwrap_err(),
                ScopeError::UnknownNetwork(_)
            ));
        }
    }

    #[test]
    fn from_wire_unknown_on_exceed_errors() {
        let wire = ScopeWire {
            on_exceed: Some("maybe".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            Scope::from_wire(wire, "/repo").unwrap_err(),
            ScopeError::UnknownOnExceed(_)
        ));
    }

    // ── SCOPE-11 from_trogon_md ───────────────────────────────────────────

    #[test]
    fn from_trogon_md_happy_path() {
        let md = "\
[scope]
scope.write: src/**, tests/**
scope.run: cargo, git
scope.protected: **/.env
scope.network: allow:api.github.com, example.com
scope.on_exceed: deny
";
        let wire = ScopeWire::from_trogon_md(md);
        assert_eq!(wire.write, vec!["src/**", "tests/**"]);
        assert_eq!(wire.run, vec!["cargo", "git"]);
        assert_eq!(wire.protected, vec!["**/.env"]);
        assert_eq!(
            wire.network,
            Some("allow:api.github.com, example.com".to_string())
        );
        assert_eq!(wire.on_exceed, Some("deny".to_string()));
    }

    #[test]
    fn from_trogon_md_fenced_code_is_ignored() {
        let md = "\
```yaml
scope.write: src/**
scope.run: cargo
scope.protected: **/.env
scope.network: on
scope.on_exceed: deny
```
";
        let wire = ScopeWire::from_trogon_md(md);
        assert_eq!(wire, ScopeWire::default());
    }

    #[test]
    fn from_trogon_md_partial_keys() {
        let md = "scope.write: src/**\nscope.network: off\n";
        let wire = ScopeWire::from_trogon_md(md);
        assert_eq!(wire.write, vec!["src/**"]);
        assert_eq!(wire.network, Some("off".to_string()));
        assert!(wire.run.is_empty());
        assert!(wire.protected.is_empty());
        assert!(wire.on_exceed.is_none());
    }

    #[test]
    fn from_trogon_md_no_scope_keys_returns_default() {
        let md = "# Project\n\n## Permissions\nallow_paths: src/**\n";
        let wire = ScopeWire::from_trogon_md(md);
        assert_eq!(wire, ScopeWire::default());
    }

    #[test]
    fn from_trogon_md_end_to_end_builds_scope() {
        let md = "\
[scope]
scope.write: src/**
scope.run: cargo
scope.network: on
scope.on_exceed: deny
";
        let wire = ScopeWire::from_trogon_md(md);
        let scope = Scope::from_wire(wire, "/repo").expect("valid scope from trogon md");
        assert!(scope.write().matches("/repo/src/main.rs"));
        assert!(scope.run().matches("cargo build"));
        assert_eq!(scope.network(), &NetworkPolicy::Allowed);
        assert_eq!(scope.on_exceed(), OnExceed::Deny);
    }

    #[test]
    fn scope_wire_deserializes_from_json_with_missing_fields() {
        let wire: ScopeWire =
            serde_json::from_str(r#"{"write":["src/**"],"on_exceed":"deny"}"#).expect("json");
        assert_eq!(wire.write, vec!["src/**".to_string()]);
        assert!(wire.run.is_empty());
        assert!(wire.network.is_none());
        let scope = Scope::from_wire(wire, "/repo").unwrap();
        assert_eq!(scope.on_exceed(), OnExceed::Deny);
    }

    // ── SCOPE-1 primitives ────────────────────────────────────────────────

    #[test]
    fn globset_empty_matches_nothing() {
        let set = GlobSet::empty();
        assert!(set.is_empty());
        assert!(!set.matches("anything/at/all"));
        assert!(set.sources().is_empty());
    }

    #[test]
    fn globset_star_and_doublestar_match_everything() {
        for pattern in ["*", "**"] {
            let set = GlobSet::compile(&[pattern.to_string()]).unwrap();
            assert!(set.matches("/a/b/c"), "`{pattern}` should match all");
            assert!(set.matches("x"));
        }
    }

    #[test]
    fn globset_specific_pattern_matches_only_its_paths() {
        let set = GlobSet::compile(&["src/**".to_string()]).unwrap();
        assert!(set.matches("src/a/b.rs"));
        assert!(!set.matches("lib/a.rs"));
        assert!(!set.is_empty());
        assert_eq!(set.sources(), &["src/**".to_string()]);
    }

    #[test]
    fn globset_invalid_pattern_errors() {
        let err = GlobSet::compile(&["[".to_string()]).unwrap_err();
        assert!(matches!(err, ScopeError::InvalidGlob { .. }));
    }

    #[test]
    fn globset_equality_is_by_sources_and_match_all() {
        let a = GlobSet::compile(&["src/**".to_string()]).unwrap();
        let b = GlobSet::compile(&["src/**".to_string()]).unwrap();
        let c = GlobSet::compile(&["tests/**".to_string()]).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn commandset_any_matches_everything_empty_matches_nothing() {
        let any = CommandSet::any();
        assert!(!any.is_empty());
        assert!(any.matches("rm -rf /"));
        assert!(any.matches("cargo test"));

        let none = CommandSet::empty();
        assert!(none.is_empty());
        assert!(!none.matches("ls"));
    }

    #[test]
    fn commandset_prefix_matching_respects_word_boundary() {
        let set = CommandSet::from_patterns(vec!["cargo".to_string(), "git".to_string()]);
        assert!(set.matches("cargo build")); // prefix + space
        assert!(set.matches("git")); // exact
        assert!(set.matches("cargo\ntest")); // prefix + newline
        assert!(!set.matches("cargojunk")); // no boundary
        assert!(!set.matches("rm")); // not listed
        assert_eq!(set.sources().len(), 2);
    }

    #[test]
    fn scope_error_display_and_source() {
        let invalid = GlobSet::compile(&["[".to_string()]).unwrap_err();
        assert!(format!("{invalid}").contains("invalid glob"));
        assert!(std::error::Error::source(&invalid).is_some());

        let net = ScopeError::UnknownNetwork("weird".to_string());
        assert!(format!("{net}").contains("weird"));
        assert!(std::error::Error::source(&net).is_none());

        let on_exceed = ScopeError::UnknownOnExceed("maybe".to_string());
        assert!(format!("{on_exceed}").contains("maybe"));
        assert!(std::error::Error::source(&on_exceed).is_none());
    }

    #[test]
    fn enums_serde_round_trip_snake_case() {
        assert_eq!(serde_json::to_string(&OnExceed::Escalate).unwrap(), "\"escalate\"");
        assert_eq!(serde_json::to_string(&NetworkPolicy::Denied).unwrap(), "\"denied\"");
        let allow = NetworkPolicy::AllowList(vec!["h".to_string()]);
        let json = serde_json::to_string(&allow).unwrap();
        assert_eq!(serde_json::from_str::<NetworkPolicy>(&json).unwrap(), allow);
    }

    #[test]
    fn scope_decision_variants_are_distinct() {
        assert_ne!(ScopeDecision::InScope, ScopeDecision::OutOfScope);
        assert_ne!(ScopeDecision::OutOfScope, ScopeDecision::Forbidden);
    }

    // ── SCOPE-4 evaluate ──────────────────────────────────────────────────

    #[test]
    fn evaluate_baseline_classifies_reads_writes_protected() {
        let s = Scope::baseline("/repo");
        // Reads are always in scope.
        assert_eq!(
            s.evaluate("read_file", &json!({"path": "src/main.rs"}), "/repo"),
            ScopeDecision::InScope
        );
        assert_eq!(
            s.evaluate("git_status", &json!({}), "/repo"),
            ScopeDecision::InScope
        );
        assert_eq!(
            s.evaluate("bash", &json!({"command": "cat src/main.rs"}), "/repo"),
            ScopeDecision::InScope
        );
        // Protected paths are a hard stop, even for reads / read-only bash.
        assert_eq!(
            s.evaluate("read_file", &json!({"path": ".env"}), "/repo"),
            ScopeDecision::Forbidden
        );
        assert_eq!(
            s.evaluate("bash", &json!({"command": "cat .env"}), "/repo"),
            ScopeDecision::Forbidden
        );
        // Writes inside cwd are in scope; outside escalates.
        assert_eq!(
            s.evaluate("write_file", &json!({"path": "src/main.rs"}), "/repo"),
            ScopeDecision::InScope
        );
        assert_eq!(
            s.evaluate("write_file", &json!({"path": "/tmp/x"}), "/repo"),
            ScopeDecision::OutOfScope
        );
        // Baseline: run-any bash in scope; network denied; local commit allowed.
        assert_eq!(
            s.evaluate("bash", &json!({"command": "cargo test"}), "/repo"),
            ScopeDecision::InScope
        );
        assert_eq!(
            s.evaluate("fetch_url", &json!({"url": "https://x"}), "/repo"),
            ScopeDecision::OutOfScope
        );
        assert_eq!(
            s.evaluate("git_commit", &json!({}), "/repo"),
            ScopeDecision::InScope
        );
        // Unknown tool fails closed.
        assert_eq!(
            s.evaluate("frobnicate", &json!({}), "/repo"),
            ScopeDecision::OutOfScope
        );
    }

    #[test]
    fn evaluate_configured_scope_enforces_roots_network_and_run() {
        let wire = ScopeWire {
            write: vec!["src/**".to_string()],
            run: vec!["cargo".to_string()],
            network: Some("on".to_string()),
            protected: vec!["**/secret.txt".to_string()],
            on_exceed: Some("deny".to_string()),
        };
        let s = Scope::from_wire(wire, "/repo").unwrap();
        // Write roots.
        assert_eq!(
            s.evaluate("write_file", &json!({"path": "src/a.rs"}), "/repo"),
            ScopeDecision::InScope
        );
        assert_eq!(
            s.evaluate("write_file", &json!({"path": "tests/a.rs"}), "/repo"),
            ScopeDecision::OutOfScope
        );
        assert_eq!(
            s.evaluate("delete_file", &json!({"path": "src/old.rs"}), "/repo"),
            ScopeDecision::InScope
        );
        // Scope-level protected glob is a hard stop.
        assert_eq!(
            s.evaluate("write_file", &json!({"path": "src/secret.txt"}), "/repo"),
            ScopeDecision::Forbidden
        );
        // Run patterns.
        assert_eq!(
            s.evaluate("bash", &json!({"command": "cargo build"}), "/repo"),
            ScopeDecision::InScope
        );
        assert_eq!(
            s.evaluate("bash", &json!({"command": "npm install"}), "/repo"),
            ScopeDecision::OutOfScope
        );
        // Network on.
        assert_eq!(
            s.evaluate("fetch_url", &json!({"url": "https://x"}), "/repo"),
            ScopeDecision::InScope
        );
        assert_eq!(
            s.evaluate("git_push", &json!({}), "/repo"),
            ScopeDecision::InScope
        );
        // Repo root is not under src/**, so committing escalates.
        assert_eq!(
            s.evaluate("git_commit", &json!({}), "/repo"),
            ScopeDecision::OutOfScope
        );
    }

    #[test]
    fn evaluate_write_without_path_fails_closed() {
        let s = Scope::baseline("/repo");
        assert_eq!(
            s.evaluate("write_file", &json!({}), "/repo"),
            ScopeDecision::OutOfScope
        );
    }

    // ── SCOPE-5 comprehensive coverage (every classification row) ──────────

    #[test]
    fn evaluate_all_read_only_tools_are_in_scope() {
        let s = Scope::baseline("/repo");
        for tool in [
            "read_file",
            "glob",
            "list_dir",
            "grep",
            "todo_read",
            "git_status",
            "git_diff",
            "git_log",
        ] {
            assert_eq!(
                s.evaluate(tool, &json!({"path": "src/x"}), "/repo"),
                ScopeDecision::InScope,
                "{tool} should be read-only in scope"
            );
        }
    }

    #[test]
    fn evaluate_all_edit_tools_gated_by_write_root() {
        let s = Scope::baseline("/repo");
        // In-cwd writes across every edit-tool variant + path key.
        assert_eq!(
            s.evaluate("str_replace", &json!({"path": "src/a.rs"}), "/repo"),
            ScopeDecision::InScope
        );
        assert_eq!(
            s.evaluate("multi_edit", &json!({"file_path": "src/b.rs"}), "/repo"),
            ScopeDecision::InScope
        );
        assert_eq!(
            s.evaluate("notebook_edit", &json!({"notebook_path": "nb.ipynb"}), "/repo"),
            ScopeDecision::InScope
        );
        // Out-of-cwd variants escalate.
        assert_eq!(
            s.evaluate("multi_edit", &json!({"file_path": "/tmp/b.rs"}), "/repo"),
            ScopeDecision::OutOfScope
        );
        assert_eq!(
            s.evaluate("notebook_edit", &json!({"notebook_path": "/etc/nb.ipynb"}), "/repo"),
            ScopeDecision::OutOfScope
        );
    }

    #[test]
    fn evaluate_all_network_tools_follow_policy() {
        let denied = Scope::baseline("/repo");
        for tool in ["fetch_url", "web_search", "git_push", "gh"] {
            assert_eq!(
                denied.evaluate(tool, &json!({}), "/repo"),
                ScopeDecision::OutOfScope,
                "{tool} should be out of scope when network denied"
            );
        }
        let allowed = Scope::from_wire(
            ScopeWire {
                network: Some("on".to_string()),
                ..Default::default()
            },
            "/repo",
        )
        .unwrap();
        for tool in ["fetch_url", "web_search", "git_push", "gh"] {
            assert_eq!(
                allowed.evaluate(tool, &json!({}), "/repo"),
                ScopeDecision::InScope,
                "{tool} should be in scope when network on"
            );
        }
    }

    #[test]
    fn evaluate_allowlist_network_is_permitted_at_gate() {
        let s = Scope::from_wire(
            ScopeWire {
                network: Some("allow:api.github.com".to_string()),
                ..Default::default()
            },
            "/repo",
        )
        .unwrap();
        assert!(matches!(s.network(), NetworkPolicy::AllowList(_)));
        // AllowList is not Denied → permitted at the scope gate; per-host
        // enforcement is EgressPolicy's job downstream.
        assert_eq!(
            s.evaluate("fetch_url", &json!({"url": "https://api.github.com/x"}), "/repo"),
            ScopeDecision::InScope
        );
        assert_eq!(
            s.evaluate("gh", &json!({}), "/repo"),
            ScopeDecision::InScope
        );
    }

    #[test]
    fn evaluate_git_create_branch_follows_repo_writability() {
        let base = Scope::baseline("/repo");
        assert_eq!(
            base.evaluate("git_create_branch", &json!({}), "/repo"),
            ScopeDecision::InScope
        );
        let src_only = Scope::from_wire(
            ScopeWire {
                write: vec!["src/**".to_string()],
                ..Default::default()
            },
            "/repo",
        )
        .unwrap();
        assert_eq!(
            src_only.evaluate("git_create_branch", &json!({}), "/repo"),
            ScopeDecision::OutOfScope
        );
    }

    #[test]
    fn evaluate_absolute_write_glob_matches_absolute_path() {
        let s = Scope::from_wire(
            ScopeWire {
                write: vec!["/data/**".to_string()],
                ..Default::default()
            },
            "/repo",
        )
        .unwrap();
        assert_eq!(
            s.evaluate("write_file", &json!({"path": "/data/out.txt"}), "/repo"),
            ScopeDecision::InScope
        );
        // A cwd-relative write is not under the absolute /data/** root.
        assert_eq!(
            s.evaluate("write_file", &json!({"path": "src/x.rs"}), "/repo"),
            ScopeDecision::OutOfScope
        );
    }

    #[test]
    fn evaluate_empty_run_blocks_write_bash_but_not_reads() {
        let s = Scope::from_wire(ScopeWire::default(), "/repo").unwrap();
        // Empty run set → every write-bash command escalates.
        assert_eq!(
            s.evaluate("bash", &json!({"command": "cargo test"}), "/repo"),
            ScopeDecision::OutOfScope
        );
        // Read-only bash is decided before the run check, so it stays in scope.
        assert_eq!(
            s.evaluate("bash", &json!({"command": "ls"}), "/repo"),
            ScopeDecision::InScope
        );
    }

    #[test]
    fn evaluate_unclassified_side_effect_tools_fail_closed() {
        let s = Scope::baseline("/repo");
        for tool in ["todo_write", "change_directory", "some_mcp_tool"] {
            assert_eq!(
                s.evaluate(tool, &json!({}), "/repo"),
                ScopeDecision::OutOfScope,
                "{tool} should fail closed"
            );
        }
    }

    // ── SCOPE-6 serde persistence ─────────────────────────────────────────

    #[test]
    fn scope_serde_round_trips_through_sources() {
        let wire = ScopeWire {
            write: vec!["src/**".to_string()],
            run: vec!["cargo".to_string()],
            network: Some("allow:api.github.com".to_string()),
            protected: vec!["**/secret".to_string()],
            on_exceed: Some("deny".to_string()),
        };
        let scope = Scope::from_wire(wire, "/repo").unwrap();
        let json = serde_json::to_string(&scope).unwrap();
        let back: Scope = serde_json::from_str(&json).unwrap();
        assert_eq!(scope, back);
        // Behavior survives the round-trip, not just the source strings.
        assert_eq!(
            back.evaluate("write_file", &json!({"path": "src/a.rs"}), "/repo"),
            ScopeDecision::InScope
        );
    }

    #[test]
    fn scope_baseline_serde_round_trips() {
        let scope = Scope::baseline("/repo");
        let json = serde_json::to_string(&scope).unwrap();
        let back: Scope = serde_json::from_str(&json).unwrap();
        assert_eq!(scope, back);
    }

    // ── SCOPE-12/13 settings.json + precedence ────────────────────────────

    #[test]
    fn from_settings_value_happy_path() {
        let settings = json!({
            "scope": {
                "write": ["src/**"],
                "run": ["cargo"],
                "network": "on",
                "on_exceed": "deny"
            }
        });
        let wire = ScopeWire::from_settings_value(&settings);
        assert_eq!(wire.write, vec!["src/**".to_string()]);
        assert_eq!(wire.run, vec!["cargo".to_string()]);
        assert_eq!(wire.network, Some("on".to_string()));
        assert_eq!(wire.on_exceed, Some("deny".to_string()));
    }

    #[test]
    fn from_settings_value_missing_scope_returns_default() {
        let settings = json!({"model": "claude-sonnet"});
        assert_eq!(ScopeWire::from_settings_value(&settings), ScopeWire::default());
    }

    #[test]
    fn from_settings_value_non_object_scope_returns_default() {
        for settings in [
            json!({"scope": "off"}),
            json!({"scope": null}),
            json!({"scope": ["src/**"]}),
        ] {
            assert_eq!(
                ScopeWire::from_settings_value(&settings),
                ScopeWire::default(),
                "expected default for {settings}"
            );
        }
    }

    #[test]
    fn from_settings_value_partial_fields() {
        let settings = json!({
            "scope": {
                "write": ["src/**"],
                "network": "off"
            }
        });
        let wire = ScopeWire::from_settings_value(&settings);
        assert_eq!(wire.write, vec!["src/**".to_string()]);
        assert_eq!(wire.network, Some("off".to_string()));
        assert!(wire.run.is_empty());
        assert!(wire.protected.is_empty());
        assert!(wire.on_exceed.is_none());
    }

    fn session_scope() -> Scope {
        Scope::from_wire(
            ScopeWire {
                network: Some("on".to_string()),
                on_exceed: Some("deny".to_string()),
                ..Default::default()
            },
            "/repo",
        )
        .expect("valid session scope")
    }

    fn settings_wire() -> ScopeWire {
        ScopeWire {
            network: Some("allow:api.github.com".to_string()),
            on_exceed: Some("escalate".to_string()),
            ..Default::default()
        }
    }

    fn trogon_wire() -> ScopeWire {
        ScopeWire {
            network: Some("off".to_string()),
            on_exceed: Some("deny".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn resolve_session_beats_settings_trogon_md_and_baseline() {
        let session = session_scope();
        let scope = Scope::resolve(
            Some(session.clone()),
            Some(settings_wire()),
            Some(trogon_wire()),
            "/repo",
        )
        .unwrap();
        assert_eq!(scope, session);
        assert_eq!(scope.network(), &NetworkPolicy::Allowed);
        assert_eq!(scope.on_exceed(), OnExceed::Deny);
    }

    #[test]
    fn resolve_settings_beats_trogon_md_and_baseline() {
        let scope = Scope::resolve(None, Some(settings_wire()), Some(trogon_wire()), "/repo")
            .unwrap();
        assert_eq!(
            scope.network(),
            &NetworkPolicy::AllowList(vec!["api.github.com".to_string()])
        );
        assert_eq!(scope.on_exceed(), OnExceed::Escalate);
    }

    #[test]
    fn resolve_trogon_md_beats_baseline() {
        let scope = Scope::resolve(None, None, Some(trogon_wire()), "/repo").unwrap();
        assert_eq!(scope.network(), &NetworkPolicy::Denied);
        assert_eq!(scope.on_exceed(), OnExceed::Deny);
        assert!(scope.run().is_empty());
    }

    #[test]
    fn resolve_falls_back_to_baseline() {
        let scope = Scope::resolve(None, None, None, "/repo").unwrap();
        let baseline = Scope::baseline("/repo");
        assert_eq!(scope, baseline);
        assert_eq!(scope.network(), &NetworkPolicy::Denied);
        assert_eq!(scope.on_exceed(), OnExceed::Escalate);
        assert!(scope.run().matches("cargo test"));
    }

    #[test]
    fn resolve_empty_settings_wire_beats_baseline() {
        let scope = Scope::resolve(None, Some(ScopeWire::default()), None, "/repo").unwrap();
        assert_ne!(scope, Scope::baseline("/repo"));
        assert!(scope.write().is_empty());
        assert!(scope.run().is_empty());
        assert!(!scope.run().matches("cargo test"));
        assert_eq!(scope.network(), &NetworkPolicy::Denied);
        assert_eq!(scope.on_exceed(), OnExceed::Escalate);
    }

    #[test]
    fn resolve_propagates_invalid_glob_from_settings() {
        let wire = ScopeWire {
            write: vec!["[".to_string()],
            ..Default::default()
        };
        let err = Scope::resolve(None, Some(wire), None, "/repo").unwrap_err();
        assert!(matches!(err, ScopeError::InvalidGlob { .. }));
    }

    #[test]
    fn resolve_propagates_invalid_glob_from_trogon_md() {
        let wire = ScopeWire {
            write: vec!["[".to_string()],
            ..Default::default()
        };
        let err = Scope::resolve(None, None, Some(wire), "/repo").unwrap_err();
        assert!(matches!(err, ScopeError::InvalidGlob { .. }));
    }
}
