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
/// (forthcoming) `baseline` / `from_wire` builders — fields are private so an
/// invalid `Scope` is unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    write: GlobSet,
    run: CommandSet,
    network: NetworkPolicy,
    protected: GlobSet,
    on_exceed: OnExceed,
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
}

/// Untrusted wire form of a [`Scope`], deserialized from TROGON.md / settings.json
/// / NATS. Converted into a domain [`Scope`] exactly once via [`Scope::from_wire`].
///
/// Field grammars:
/// - `network`: `off` | `on` | `allow:host1,host2,...` (absent => denied)
/// - `on_exceed`: `escalate` | `deny` (absent => escalate)
#[derive(Debug, Default, Deserialize)]
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
}
