use std::time::Duration;

/// Keychain service that holds the Claude Code OAuth credentials on macOS.
pub(crate) const KEYCHAIN_SERVICE_NAME: &str = "Claude Code-credentials";

/// Upper bound for the keychain lookup that copies credentials into a resumed session.
pub(crate) const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Keychain account used when `USER` is not set, matching the TypeScript SDK.
pub(crate) const KEYCHAIN_FALLBACK_ACCOUNT: &str = "claude-code-user";

/// Settings keys removed from the copied `settings.json` of a resumed session.
pub(crate) const RESUME_SETTINGS_STRIPPED_KEYS: [&str; 2] = ["enabledPlugins", "extraKnownMarketplaces"];

/// Attempts to remove a materialized resume directory before giving up.
pub(crate) const CLEANUP_ATTEMPTS: usize = 4;

/// Delay between attempts to remove a materialized resume directory.
pub(crate) const CLEANUP_DELAY: Duration = Duration::from_millis(100);

/// Backoff before each retry of a failed transcript mirror append.
pub(crate) const APPEND_BACKOFF_MS: [u64; 2] = [200, 800];

/// Permission update `type` values this crate decodes into typed variants.
pub(crate) const KNOWN_UPDATE_TYPES: [&str; 6] = [
    "addRules",
    "replaceRules",
    "removeRules",
    "setMode",
    "addDirectories",
    "removeDirectories",
];

/// Protocol versions the pinned TypeScript SDK negotiates, newest first.
pub(crate) const SUPPORTED_PROTOCOL_VERSIONS: [&str; 5] =
    ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05", "2024-10-07"];

pub(crate) const MAX_CLIENT_APP_LENGTH: usize = 128;

pub(crate) const GENERIC_NAMES: [&str; 6] = ["index", "main", "cli", "server", "app", "node"];

pub const DEFAULT_ENTRYPOINT: &str = "sdk-trogon-rs";

pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Inherited variables the plan consults. Captured through the environment adapter before spawning.
pub(crate) const INHERITED_KEYS: &[&str] = &[
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_CODE_SDK_READS_SESSION_STATE",
    "DEBUG_CLAUDE_AGENT_SDK",
    "CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK",
    "PATH",
    "HOME",
    "USERPROFILE",
];

/// Upstream `_STORE_LIST_LOAD_CONCURRENCY`.
pub(crate) const LIST_LOAD_CONCURRENCY: usize = 16;

/// Upstream `MAX_PENDING_ENTRIES` and `MAX_PENDING_BYTES`, the import batch defaults.
pub(crate) const DEFAULT_IMPORT_BATCH: usize = 500;

pub(crate) const MAX_IMPORT_BYTES: usize = 1024 * 1024;

pub(crate) const MAX_SANITIZED_LENGTH: usize = 200;

/// Size of the head and tail buffers of a lite metadata read.
pub(crate) const LITE_READ_BUF_SIZE: u64 = 65_536;

pub(crate) const TRANSCRIPT_TYPES: [&str; 5] = ["user", "assistant", "attachment", "system", "progress"];

pub(crate) const SCRIPT_EXTENSIONS: [&str; 5] = ["js", "mjs", "ts", "tsx", "jsx"];

pub(crate) const UNIX_HINT: &str = "Claude Code not found. Install with:\n  npm install -g @anthropic-ai/claude-code\n\nIf already installed locally, try:\n  export PATH=\"$HOME/node_modules/.bin:$PATH\"\n\nOr provide the path via ClaudeAgentOptions:\n  ClaudeAgentOptions(cli_path='/path/to/claude')";

pub(crate) const WINDOWS_HINT: &str = "Claude Code not found. Install the native claude.exe with (PowerShell):\n  irm https://claude.ai/install.ps1 | iex\n\nOr install the claude-agent-sdk wheel for a platform that bundles claude.exe (e.g. Windows x64), or provide the path to a claude.exe via ClaudeAgentOptions:\n  ClaudeAgentOptions(cli_path='C:\\\\path\\\\to\\\\claude.exe')\n\n(npm install -g @anthropic-ai/claude-code produces a claude.cmd shim, which this SDK refuses to run on Windows.)";

/// Oldest Claude Code release the SDK supports without a warning.
pub const MINIMUM_CLAUDE_CODE_VERSION: &str = "2.0.0";

pub(crate) const PARENT_MARK: &str = "\"parentUuid\":";

pub(crate) const CONTINUED_MARK: &str = "\"type\":\"continued-in\"";

pub(crate) const PAGE_WINDOW: usize = 32;

pub(crate) const CONTINUED_SCAN_LIMIT: u64 = 16_777_216;

pub(crate) const CONTINUED_SCAN_CHUNK: usize = 1_048_576;

pub(crate) const PRECOMPACT_CHUNK: usize = 1_048_576;

/// Files larger than this take the pre-compact skip (`p0`).
pub(crate) const PRECOMPACT_SKIP_THRESHOLD: u64 = 5_242_880;

pub(crate) const PRECOMPACT_NEEDLE: &[u8] = b"\"compact_boundary\"";

pub(crate) const PRECOMPACT_SNAPSHOT: &[u8] = b"{\"type\":\"attribution-snapshot\"";

pub(crate) const PRECOMPACT_SYSTEM: &[u8] = b"{\"type\":\"system\"";

pub(crate) const PRECOMPACT_NEAR: usize = 256;

pub(crate) const PRECOMPACT_NEWLINE: u8 = 10;

pub(crate) const TS_REPLY_PREFIXES: [&str; 10] = [
    "[Request interrupted by user]",
    "[Request interrupted: the session ended before this agent finished. Nobody asked it to stop.]",
    "[Agent call interrupted: the process running this conversation stopped before this agent finished, so it returned no result. Nobody asked it to stop.]",
    "[Request interrupted by user for tool use]",
    "[Tool call did not complete: the turn was ended to deliver the message that follows. Nothing refused it; re-run it if still needed.]",
    "[Tool call interrupted: the session ended before this call's result was recorded, so its outcome is unknown. Check whether it took effect before relying on it or running it again.]",
    "[Tool call result not in this copy: this session was copied from another session before that session recorded this call's result. The call may have finished there, may still be running there, or may never have run. Check whether it took effect before relying on it or running it again.]",
    "The user doesn't want to take this action right now. STOP what you are doing and wait for the user to tell you how to proceed.",
    "[Tool call skipped: the turn was stopped before this call ran, by the check whose denial is on another call in this batch. Nothing refused this call and it had no effects; re-run it if still needed.]",
    "[Tool call skipped: the turn ended to deliver the message that follows before this call ran. Nothing refused it; re-run it if still needed.]",
];

pub(crate) const TS_DENIAL_KINDS: [&str; 7] = [
    "user-rejected",
    "permission-rule",
    "automode-blocked",
    "automode-unavailable",
    "automode-parsing-error",
    "interrupted",
    "cancelled",
];

/// `Exe`: the trailer appended to a plugin prompt in a stored user message.
pub(crate) const TS_PLUGIN_PROMPT_TRAILER: &str = "\n\nThis is how Claude Code surfaces a prompt a plugin submits between turns \u{2014} it starts this turn in the user's place. Address the message above.";

pub(crate) const TS_PLUGIN_PROMPT_HEADER_PREFIX: &str = "The ";

pub(crate) const TS_PLUGIN_PROMPT_HEADER_SUFFIX: &str = " plugin sent a message:\n";

pub(crate) const TS_PASTED_OPEN: &str = "<pasted_content id=\"";

pub(crate) const TS_TRANSCRIPT_KINDS: [&str; 5] = ["user", "assistant", "attachment", "system", "progress"];

/// `wMe`: the longest identifier the data-flow records accept.
pub(crate) const TS_MAX_IDENTIFIER_UNITS: usize = 256;

pub(crate) const TS_FLOW_VERSION: u64 = 1;

/// `kMe`: exclusive bound of `inner_call_index`.
pub(crate) const TS_INNER_CALL_LIMIT: f64 = 2_147_483_648.0;

pub(crate) const TS_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

pub(crate) const TS_BASE58_ALPHABET: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

pub(crate) const TS_FLOW_TOUCHED_KINDS: [&str; 3] = ["artifact", "memory", "session"];

pub(crate) const TS_FLOW_TOUCHED_ACTIONS: [&str; 3] = ["read", "write", "copy"];

pub(crate) const TS_DERIVED_TYPES: [&str; 2] = ["claude_code_session", "unknown"];

pub(crate) const TS_DERIVED_VIA: [&str; 11] = [
    "clear",
    "fork",
    "branch",
    "background",
    "cloud",
    "teleport",
    "teammate",
    "launch",
    "resume",
    "import",
    "compact",
];

/// `MB`: keys a fork clears on every copied entry (`isSidechain` is reset to false separately).
pub(crate) const TS_FORK_CLEARED_KEYS: [&str; 8] = [
    "teamName",
    "agentName",
    "sessionKind",
    "slug",
    "sourceToolAssistantUUID",
    "summarizedTurnPromptUuids",
    "droppedPromptUuids",
    "turnAttributionKey",
];

pub(crate) const TS_FORK_MESSAGE_KEYS: [&str; 3] = ["anchorUuid", "uuids", "allUuids"];

pub(crate) const TS_FORK_SEGMENT_KEYS: [&str; 3] = ["headUuid", "anchorUuid", "tailUuid"];

pub(crate) const TS_FORK_TITLE_FALLBACK: &str = "Forked session";
