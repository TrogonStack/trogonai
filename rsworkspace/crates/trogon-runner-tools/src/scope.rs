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
        let root = format!("{}/**", globset::escape(cwd.trim_end_matches('/')));
        let write = GlobSet::compile(&[root]).expect("cwd-anchored `**` is a valid glob");
        Self {
            write,
            run: CommandSet::any(),
            network: NetworkPolicy::Denied,
            protected: GlobSet::empty(),
            on_exceed: OnExceed::Escalate,
        }
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
}
