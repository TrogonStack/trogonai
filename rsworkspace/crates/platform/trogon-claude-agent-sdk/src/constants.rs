use std::ops::RangeInclusive;
use std::time::Duration;

/// Keychain service that holds the Claude Code OAuth credentials on macOS.
pub(crate) const KEYCHAIN_SERVICE_NAME: &str = "Claude Code-credentials";

/// Upper bound for the keychain lookup that copies credentials into a resumed session.
pub(crate) const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Keychain account used when `USER` is not set, matching the TypeScript SDK.
pub(crate) const KEYCHAIN_FALLBACK_ACCOUNT: &str = "claude-code-user";

/// Settings keys removed from the copied `settings.json` of a resumed session.
pub(crate) const RESUME_SETTINGS_STRIPPED_KEYS: [&str; 2] = ["enabledPlugins", "extraKnownMarketplaces"];

/// The settings keys the TypeScript SDK strips from a resumed session's settings files.
pub(crate) const RESUME_SETTINGS_STRIPPED_KEYS_TS: [&str; 3] =
    ["enabledPlugins", "extraKnownMarketplaces", "additionalMarketplaces"];

/// The TypeScript SDK's per-environment OAuth file name suffixes beyond the default `.claude.json`.
pub(crate) const OAUTH_FILE_SUFFIXES_TS: [&str; 3] = ["-staging-oauth", "-local-oauth", "-custom-oauth"];

/// The custom OAuth endpoints the TypeScript SDK approves for `CLAUDE_CODE_CUSTOM_OAUTH_URL`.
pub(crate) const APPROVED_OAUTH_ENDPOINTS_TS: [&str; 3] = [
    "https://beacon.claude-ai.staging.ant.dev",
    "https://claude.fedstart.com",
    "https://claude-staging.fedstart.com",
];

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

/// Identity the derived client app label carries, standing in for the Python distribution metadata.
pub(crate) const SDK_PACKAGE_NAME: &str = env!("CARGO_PKG_NAME");

pub(crate) const CLIENT_APP_ENV: &str = "CLAUDE_AGENT_SDK_CLIENT_APP";

pub(crate) const DERIVED_CLIENT_APP_ENV: &str = "CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP";

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

/// `cmd.exe` metacharacters Python refuses in resume and session values on Windows.
pub(crate) const WINDOWS_CMD_METACHARACTERS: &str = "&|<>^%!\"";

/// Python `--setting-sources` default when `skills` is set and no sources were given.
pub(crate) const SKILLS_DEFAULT_SETTING_SOURCES: [&str; 2] = ["user", "project"];

/// Environment variable that makes the TypeScript SDK log to a derived debug file.
pub(crate) const SDK_DEBUG_ENV: &str = "DEBUG_CLAUDE_AGENT_SDK";

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

/// Largest settings file the resolver reads, in bytes (`fm` upstream).
pub(crate) const SETTINGS_FILE_MAX_BYTES: usize = 2_097_152;

/// Largest `plutil` or `reg.exe` output the resolver reads, in bytes (`Sy` upstream).
pub(crate) const POLICY_READ_MAX_BUFFER: usize = 2_097_152;

/// Timeout for each `plutil` or `reg.exe` run (`Ex` upstream).
pub(crate) const POLICY_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Keys that do not make a managed settings document count as policy content (`Qi` upstream).
pub(crate) const POLICY_META_KEYS: [&str; 2] = ["managedSourcesBehavior", "wslInheritsWindowsSettings"];

/// Blocks whose fail-closed reading never counts as a stand-in for policy content (`K3` upstream).
pub(crate) const POLICY_UNSYNTHESIZED_BLOCKS: [&str; 2] = ["remoteTools", "remoteControl"];

/// Locks a managed document never replaces with their restrictive value (`neverSubstitute` upstream).
pub(crate) const POLICY_NEVER_SUBSTITUTED: [&str; 1] = ["sandbox.failIfUnavailable"];

/// Locks left out of the restrictive skeleton of a block that is not an object (`skeletonExclude` upstream).
pub(crate) const POLICY_SKELETON_EXCLUDED: [&str; 1] = ["sandbox.enabled"];

/// Prefix of the variable names a managed AWS pair that cannot be read is given (`Zx` upstream).
pub(crate) const POLICY_INVALID_PAIR_PREFIX: &str = "_INVALID_PAIR_";

/// FNV-1a offset basis of the managed file hash in stand-in pair names.
pub(crate) const POLICY_FILE_HASH_OFFSET: u32 = 2_166_136_261;

/// FNV-1a prime of the managed file hash in stand-in pair names.
pub(crate) const POLICY_FILE_HASH_PRIME: u32 = 16_777_619;

/// Keys only honored from administrator-controlled sources (`mE` upstream).
pub(crate) const MANAGED_ONLY_KEYS: [&str; 3] = ["managedMcpServers", "isolation", "attributionProjectPolicy"];

/// Settings sources in precedence order, lowest first (`x5` upstream).
pub(crate) const SOURCE_ORDER: [&str; 5] = [
    "userSettings",
    "projectSettings",
    "localSettings",
    "flagSettings",
    "policySettings",
];

/// Default `settingSources` of `resolveSettings` (`Cet` upstream).
pub(crate) const DEFAULT_SETTING_SOURCES: [&str; 3] = ["user", "project", "local"];

/// `permissions.defaultMode` values the trust filter treats as escalating (`Aet` upstream).
pub(crate) const ESCALATING_DEFAULT_MODES: [&str; 3] = ["bypassPermissions", "auto", "acceptEdits"];

/// Escalating modes that a repo-local `local` source may also not set (`P5` upstream).
pub(crate) const STRICT_ESCALATING_DEFAULT_MODES: [&str; 2] = ["auto", "bypassPermissions"];

/// Bundle identifier whose managed preferences hold the policy.
pub(crate) const PLIST_DOMAIN: &str = "com.anthropic.claudecode";

/// Directory that holds managed preferences on macOS.
pub(crate) const PLIST_DIRECTORY: &str = "/Library/Managed Preferences";

/// `plutil` binary the plist reader runs.
pub(crate) const PLUTIL_PATH: &str = "/usr/bin/plutil";

/// Arguments of the `plutil` conversion to JSON.
pub(crate) const PLUTIL_CONVERT_ARGS: [&str; 5] = ["-convert", "json", "-o", "-", "--"];

/// Arguments of the `plutil` lint run.
pub(crate) const PLUTIL_LINT_ARGS: [&str; 3] = ["-lint", "-s", "--"];

/// Machine-wide registry key of the Windows policy.
pub(crate) const REGISTRY_MACHINE_KEY: &str = "HKLM\\SOFTWARE\\Policies\\ClaudeCode";

/// Per-user registry key of the Windows policy.
pub(crate) const REGISTRY_USER_KEY: &str = "HKCU\\SOFTWARE\\Policies\\ClaudeCode";

/// Registry value that holds the settings document.
pub(crate) const REGISTRY_VALUE_NAME: &str = "Settings";

/// What upstream calls the managed settings a host passes in when it reads them (`iX`).
pub(crate) const PARENT_MANAGED_FILE: &str = "parent managed settings";

/// What upstream calls the server-managed settings when it reads them.
pub(crate) const REMOTE_MANAGED_FILE: &str = "remote managed settings";

/// What upstream calls the signed-in user's managed preferences document when it reads it (`Cx`).
pub(crate) const PER_USER_PLIST_FILE: &str = "per-user managed preferences";

/// What upstream calls the device-level managed preferences document when it reads it.
pub(crate) const DEVICE_PLIST_FILE: &str = "device-level managed preferences";

/// What upstream calls the machine-wide registry document when it reads it.
pub(crate) const REGISTRY_MACHINE_FILE: &str = "Registry: HKLM\\SOFTWARE\\Policies\\ClaudeCode\\Settings";

/// What upstream calls the per-user registry document when it reads it.
pub(crate) const REGISTRY_USER_FILE: &str = "Registry: HKCU\\SOFTWARE\\Policies\\ClaudeCode\\Settings";

/// Text `reg query` prints when the key or value does not exist.
pub(crate) const REGISTRY_NOT_FOUND_TEXT: &str = "unable to find the specified registry key or value";

/// `reg.exe` as WSL reaches it.
pub(crate) const WSL_REGISTRY_TOOL: &str = "/mnt/c/Windows/System32/reg.exe";

/// Windows managed settings directory as WSL reaches it.
pub(crate) const WSL_MANAGED_DIRECTORY: &str = "/mnt/c/Program Files/ClaudeCode";

/// Kernel version file the platform check reads.
pub(crate) const PROC_VERSION_PATH: &str = "/proc/version";

/// Managed settings directory on macOS.
pub(crate) const MACOS_MANAGED_DIRECTORY: &str = "/Library/Application Support/ClaudeCode";

/// Managed settings directory on Windows.
pub(crate) const WINDOWS_MANAGED_DIRECTORY: &str = "C:\\Program Files\\ClaudeCode";

/// Managed settings directory everywhere else.
pub(crate) const LINUX_MANAGED_DIRECTORY: &str = "/etc/claude-code";

/// Managed settings file name.
pub(crate) const MANAGED_SETTINGS_FILE: &str = "managed-settings.json";

/// Managed settings drop-in directory name.
pub(crate) const MANAGED_SETTINGS_DROP_IN_DIRECTORY: &str = "managed-settings.d";

/// Managed policy key that decides whether tiers merge or the first one wins.
pub(crate) const MANAGED_SOURCES_BEHAVIOR_KEY: &str = "managedSourcesBehavior";

/// Environment variable that opts out of the admin environment union.
pub(crate) const DISABLE_ADMIN_ENV_UNION_VARIABLE: &str = "CLAUDE_CODE_DISABLE_ADMIN_ENV_UNION";

/// Boolean policy keys a parent `managedSettings` can only tighten to `true`.
pub(crate) const PARENT_RESTRICT_TRUE_KEYS: [&str; 4] = [
    "disableRemoteControl",
    "disableAgentView",
    "disableWorkflows",
    "disableArtifact",
];

/// Boolean policy keys a parent `managedSettings` can only tighten to `false`.
pub(crate) const PARENT_RESTRICT_FALSE_KEYS: [&str; 2] = ["enableWorkflows", "enableArtifact"];

/// Allow-list policy keys that a parent slice never overrides (`Mu` upstream).
pub(crate) const ALLOW_LIST_KEYS: [&str; 4] = [
    "allowedMcpServers",
    "availableModels",
    "strictKnownMarketplaces",
    "allowedProviders",
];

/// The settings schema extracted from the pinned upstream bundle (`claude-sdk:schema`).
pub(crate) const SETTINGS_SCHEMA_JSON: &str = include_str!("upstream/settings_schema.json");

/// Magnitude up to which every integer is exactly a JavaScript number (2^53).
pub(crate) const JS_MAX_EXACT_INTEGER: u64 = 1 << 53;

/// The longest array JavaScript holds (2^32 - 1).
pub(crate) const JS_MAX_ARRAY_LENGTH: u64 = u32::MAX as u64;

/// The most UTF-16 units a V8 string holds (`String::kMaxLength`, 2^29 - 24); building a longer one throws
/// `RangeError: Invalid string length`.
pub(crate) const V8_MAX_STRING_LENGTH: u64 = (1 << 29) - 24;

/// The double a settings value holds for `Infinity` (2^63). `serde_json` has no number for Infinity, and number
/// normalization turns every whole double below 2^64 that a document holds into an integer, so no document
/// leaves a float with this value behind.
pub(crate) const SETTINGS_INFINITY_MARKER: f64 = 9_223_372_036_854_775_808.0;

/// The double a settings value holds for `-Infinity` (-2^62), for the same reason as [`SETTINGS_INFINITY_MARKER`].
pub(crate) const SETTINGS_NEGATIVE_INFINITY_MARKER: f64 = -4_611_686_018_427_387_904.0;

/// The integer equal to [`SETTINGS_INFINITY_MARKER`]. A finite double with the marker's value, such as a document's
/// `9223372036854775808.0`, is held as this integer so that it does not read back as `Infinity`.
pub(crate) const SETTINGS_INFINITY_MARKER_INTEGER: u64 = 1 << 63;

/// The integer equal to [`SETTINGS_NEGATIVE_INFINITY_MARKER`], for the same reason as
/// [`SETTINGS_INFINITY_MARKER_INTEGER`].
pub(crate) const SETTINGS_NEGATIVE_INFINITY_MARKER_INTEGER: i64 = -(1 << 62);

/// The double the first exact Python number of a resolve stands for (2^62). The n-th one is this plus
/// `n * SETTINGS_PYTHON_NUMBER_MARKER_STEP`, below [`SETTINGS_INFINITY_MARKER`]. A whole double below 2^64 that a
/// document holds is normalized into an integer, so only a marker is left as a float in this range.
pub(crate) const SETTINGS_PYTHON_NUMBER_MARKER_BASE: f64 = 4_611_686_018_427_387_904.0;

/// The distance between two consecutive Python number markers; doubles from 2^62 to 2^63 are 1024 apart.
pub(crate) const SETTINGS_PYTHON_NUMBER_MARKER_STEP: f64 = 2048.0;

/// The first surrogate code point, U+D800.
pub(crate) const SURROGATE_FIRST: u32 = 0xD800;

/// Where each range of private-use code points a lone surrogate of a Python-dialect settings string first tries to
/// stand as begins: U+D800 + n tries the start plus n of each in turn. Each range is as wide as the surrogates and
/// lies in the Basic Multilingual Plane, so a stand-in is one UTF-16 unit, as the surrogate is.
pub(crate) const SETTINGS_SURROGATE_STAND_IN_STARTS: [u32; 3] = [0xE000, 0xE800, 0xF000];

/// The private-use code points a lone surrogate of a Python-dialect settings string may stand as, in the order they
/// are tried after its place in each of [`SETTINGS_SURROGATE_STAND_IN_STARTS`]: the Basic Multilingual Plane's,
/// one UTF-16 unit each, then those of planes 15 and 16, two units each.
pub(crate) const SETTINGS_SURROGATE_STAND_IN_RANGES: [(u32, u32); 3] =
    [(0xE000, 0xF8FF), (0xF_0000, 0xF_FFFD), (0x10_0000, 0x10_FFFD)];

/// The most decimal digits Python's `int()` converts from a string (`sys.get_int_max_str_digits()`).
pub(crate) const PYTHON_INT_MAX_STR_DIGITS: usize = 4300;

/// CPython 3.12's `Py_C_RECURSION_LIMIT`: how many C recursion checks may be open at once. The C scanner of
/// `json.loads` opens one for each array and object it enters, and `sys.setrecursionlimit` does not move the limit.
pub(crate) const PYTHON_C_RECURSION_LIMIT: usize = 10_000;

/// The stack of the thread the settings engine runs the Python dialect on. The Python dialect reads a settings file
/// nested as deep as `json.loads` reads it, almost [`PYTHON_C_RECURSION_LIMIT`] levels, and the engine walks the
/// document recursively, a few frames per level at each step. Untouched stack pages are never committed, so the
/// reservation costs address space, not memory.
pub(crate) const SETTINGS_ENGINE_STACK_BYTES: usize = 256 * 1024 * 1024;

/// The most arrays and objects `serde_json` reads nested in one document before it stops with "recursion limit
/// exceeded".
pub(crate) const SERDE_JSON_NESTING: usize = 127;

/// Largest integer a JavaScript number holds exactly (`Number.MAX_SAFE_INTEGER`).
pub(crate) const SETTINGS_SCHEMA_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// The extension sections an upstream array setting accepts (`Qf`).
pub(crate) const SETTINGS_SCHEMA_ARRAY_SECTIONS: [&str; 4] = ["skills", "agents", "hooks", "mcp"];

pub(crate) const USER_SETTINGS_FILE: &str = "settings.json";
pub(crate) const PROJECT_SETTINGS_FILE: [&str; 2] = [".claude", "settings.json"];
pub(crate) const LOCAL_SETTINGS_FILE: [&str; 2] = [".claude", "settings.local.json"];
pub(crate) const BUSY_ERRNOS: [&str; 4] = ["EAGAIN", "EMFILE", "ENFILE", "ENOMEM"];
pub(crate) const MAX_BUFFER_ERRNO: &str = "ERR_CHILD_PROCESS_STDIO_MAXBUFFER";
pub(crate) const OTEL_PREFIXES: [&str; 2] = ["OTEL_EXPORTER_OTLP_", "ANT_OTEL_EXPORTER_OTLP_"];
pub(crate) const OTEL_EXACT: [&str; 10] = [
    "OTEL_LOG_RAW_API_BODIES",
    "OTEL_LOG_USER_PROMPTS",
    "OTEL_LOG_ASSISTANT_RESPONSES",
    "OTEL_LOG_TOOL_CONTENT",
    "OTEL_LOG_TOOL_DETAILS",
    "OTEL_LOG_MANAGED_SETTINGS",
    "OTEL_LOGS_EXPORTER",
    "ENABLE_BETA_TRACING_DETAILED",
    "BETA_TRACING_ENDPOINT",
    "ANT_OTEL_LOGS_EXPORTER",
];
pub(crate) const AUTH_HELPERS: [&str; 4] = [
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "gcpAuthRefresh",
];
pub(crate) const CLOUD_PROVIDER_FLAGS: [&str; 3] = [
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_MANTLE",
];

/// Upstream `aM` gives up on a path after this many symlink hops.
pub(crate) const SYMLINK_HOP_LIMIT: usize = 64;

/// Directory beside the Python package that holds the bundled Claude Code executable.
pub(crate) const BUNDLED_DIRECTORY: &str = "_bundled";

/// npm package name shared by the TypeScript SDK and its platform packages.
pub(crate) const PACKAGE_SCOPE_NAME: &str = "@anthropic-ai/claude-agent-sdk";

/// File beside the bundled executable that records the version the CLI reported at build time.
pub(crate) const BUNDLED_CLI_VERSION_FILE: &str = "claude.version";

/// First Claude Code version that keeps the derived client app label from what it spawns. Python keeps it above
/// every release until one does, so no label is derived before then.
pub(crate) const DERIVED_CLIENT_APP_MINIMUM_CLAUDE_CODE_VERSION: [u32; 3] = [999, 0, 0];

/// Scheme prefixes (lower case) that mark a marketplace source as remote.
pub(crate) const SETTINGS_SCHEMA_REMOTE_SCHEMES: [&str; 11] = [
    "file:",
    "http:",
    "https:",
    "git:",
    "git+http:",
    "git+https:",
    "git+ssh:",
    "ssh:",
    "github:",
    "gitlab:",
    "bitbucket:",
];

/// First size of the `getpwnam_r` scratch buffer, doubled on `ERANGE`.
pub(crate) const PASSWD_INITIAL_BUFFER: usize = 1024;

/// Largest `getpwnam_r` scratch buffer tried before giving up.
pub(crate) const PASSWD_MAX_BUFFER: usize = 1 << 20;

/// Upstream `C5e`: control, line separator and default-ignorable characters a helper path must not hold.
pub(crate) const SETTINGS_PATTERN_INVISIBLE_PATH: &str = r"[\x00-\x1f\x7f-\x9f\x{2028}\x{2029}]|\p{DI}";

/// Upstream `R3`: control and bidirectional formatting characters.
pub(crate) const SETTINGS_PATTERN_BIDI_NAME: &str = r"[\p{Cc}\x{200E}\x{200F}\x{202A}-\x{202E}\x{2066}-\x{2069}]";

/// Upstream `C3e` and `A3e`: control and invisible format characters.
pub(crate) const SETTINGS_PATTERN_FORMAT_CHARS: &str = r"[\p{Cc}\p{Cf}\x{2028}\x{2029}]";

/// Upstream `W6` plus the `V6` anchor: marketplace names that impersonate an official one.
pub(crate) const SETTINGS_PATTERN_IMPERSONATION: &str = "official[^a-z0-9]*(anthropic|claude)|(?:anthropic|claude)[^a-z0-9]*official|^(?:anthropic|claude)[^a-z0-9]*(marketplace|plugins|official)";

/// Upstream `G6e`: a `${NAME}` or `${NAME:-default}` environment reference.
pub(crate) const SETTINGS_PATTERN_ENV_REFERENCE: &str = r"\$\{[A-Za-z_][A-Za-z0-9_]*(?::-[^}]*)?\}";

/// Upstream `bJe`: a scoped npm package name.
pub(crate) const SETTINGS_PATTERN_SCOPED_PACKAGE: &str = r"^@[a-z0-9][a-z0-9\-._]*/[a-z0-9][a-z0-9\-._]*$";

/// Upstream `CJe`: an unscoped npm package name.
pub(crate) const SETTINGS_PATTERN_PLAIN_PACKAGE: &str = r"^[a-z0-9][a-z0-9\-._]*$";

/// Upstream `Xo` search: a macOS firmlink-style path component.
pub(crate) const SETTINGS_PATTERN_VOLUME_COMPONENT: &str = r"/\.(?:vol|file|nofollow|resolve)(?:/|$)";

/// Upstream `Xo` first-component test.
pub(crate) const SETTINGS_PATTERN_VOLUME_ROOT: &str = r"^\.(?:vol|file|nofollow|resolve)$";

/// Upstream `R5e` automount root.
pub(crate) const SETTINGS_PATTERN_NETWORK_SERVERS: &str = r"^/network/servers(/|$)";

/// Upstream `O5e` kernel magic-link roots.
pub(crate) const SETTINGS_PATTERN_MAGIC_LINK: &str = r"^/(proc|dev/(fd|stdin|stdout|stderr))(/|$)";

/// Upstream `w5e` and `v5e`: PowerShell and executable suffixes.
pub(crate) const SETTINGS_PATTERN_WINDOWS_EXECUTABLE: &str = r"\.(exe|ps1)$";

/// Upstream `v5e`.
pub(crate) const SETTINGS_PATTERN_POWERSHELL_SUFFIX: &str = r"\.ps1$";

/// Node `net.isIPv4` segment.
pub(crate) const SETTINGS_PATTERN_IPV4_SEGMENT: &str = "(?:25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9][0-9]|[0-9])";

/// Node `net.isIPv6` segment.
pub(crate) const SETTINGS_PATTERN_IPV6_SEGMENT: &str = "(?:[0-9a-fA-F]{1,4})";

/// Node `net.isIPv6` zone suffix.
pub(crate) const SETTINGS_PATTERN_IPV6_ZONE: &str = "(?:%[0-9a-zA-Z-.:]{1,})?$";

/// Upstream `K6`, `j6` and `nJe` together: marketplace names reserved for Anthropic.
pub(crate) const SETTINGS_RESERVED_MARKETPLACE_NAMES: [&str; 19] = [
    "claude-code-marketplace",
    "claude-code-plugins",
    "claude-plugins-official",
    "anthropic-marketplace",
    "anthropic-plugins",
    "agent-skills",
    "anthropic-agent-skills",
    "life-sciences",
    "knowledge-work-plugins",
    "claude-for-legal",
    "claude-for-financial-services",
    "financial-services-plugins",
    "first-party-plugins",
    "claude-tag-plugins",
    "claude-community",
    "claude-plugins-community",
    "healthcare",
    "anthropic-plugin-directory",
    "claude-plugin-directory",
];

/// Own property names of `Object.prototype`.
pub(crate) const SETTINGS_OBJECT_PROTOTYPE_NAMES: [&str; 12] = [
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];

/// Upstream `$3e`: cloud metadata and loopback host names.
pub(crate) const SETTINGS_METADATA_HOSTS: [&str; 12] = [
    "metadata.google.internal",
    "metadata.goog",
    "metadata",
    "instance-data",
    "instance-data.ec2.internal",
    "ip6-localhost",
    "ip6-loopback",
    "localhost.localdomain",
    "localhost4",
    "localhost4.localdomain4",
    "localhost6",
    "localhost6.localdomain6",
];

/// Upstream `p6`: link-local metadata addresses outside the loopback ranges.
pub(crate) const SETTINGS_METADATA_ADDRESSES: [&str; 3] = ["100.100.100.200", "168.63.129.16", "192.0.0.192"];

/// Upstream `Y6` keys: marketplace names reserved for session-scoped plugin sources.
pub(crate) const SETTINGS_BUILTIN_PLUGIN_NAMES: [&str; 5] =
    ["inline", "builtin", "skills-dir", "synced", "claude-plugin-test"];

/// Upstream `Gf`: provider names an `allowedProviders` entry may carry.
pub(crate) const SETTINGS_PROVIDER_NAMES: [&str; 8] = [
    "anthropic",
    "customEndpoint",
    "bedrock",
    "vertex",
    "foundry",
    "anthropicAws",
    "mantle",
    "gateway",
];

/// Upstream `x3e`: the prefix of the provider names an `allowedProviders` entry may carry beyond `Gf`.
pub(crate) const SETTINGS_PROVIDER_FAMILY: &str = "anthropic";

/// Upstream `E3e`: server fields a managed settings server must not carry.
pub(crate) const SETTINGS_MANAGED_SERVER_FORBIDDEN: [&str; 4] = ["command", "args", "env", "headersHelper"];

/// Upstream `b3e`: server types a managed settings server may use.
pub(crate) const SETTINGS_MANAGED_SERVER_TYPES: [&str; 3] = ["http", "streamable-http", "sse"];

/// Keys a static `defaultSettings` payload must not carry (upstream `al`: the policy helper keys, the
/// entry fields, the envelope keys and the per-OS keys; a key in two groups counts once per group).
pub(crate) const SETTINGS_PAYLOAD_FORBIDDEN_KEYS: [&str; 18] = [
    "policyHelper",
    "policyHelpers",
    "path",
    "script",
    "interpreter",
    "outputBehavior",
    "onFailure",
    "retries",
    "timeoutMs",
    "refreshIntervalMs",
    "defaultSettings",
    "managedSettings",
    "appendSystemPrompt",
    "macos",
    "linux",
    "windows",
    "wsl",
    "default",
];

/// Upstream `f3e`: the largest `timeout` a request timeout converts to.
pub(crate) const SETTINGS_REQUEST_TIMEOUT_LIMIT: f64 = 300_000.0;

/// The credential file `mode` upstream cleans malformed optional fields for.
pub(crate) const SETTINGS_CREDENTIAL_DENY_MODE: &str = "deny";

/// Upstream `gy`: legacy tool names a permission rule resolves to their current names.
pub(crate) const SETTINGS_PERMISSION_TOOL_ALIASES: [(&str, &str); 8] = [
    ("Task", "Agent"),
    ("KillShell", "TaskStop"),
    ("KillBash", "TaskStop"),
    ("ListPeers", "ListAgents"),
    ("Brief", "SendUserMessage"),
    ("ListMcpResources", "ListMcpResourcesTool"),
    ("ReadMcpResource", "ReadMcpResourceTool"),
    ("ReadMcpResourceDir", "ReadMcpResourceDirTool"),
];

/// Upstream `Zf.filePatternTools`: tools whose rule content is a file glob.
pub(crate) const SETTINGS_PERMISSION_FILE_PATTERN_TOOLS: [&str; 7] =
    ["Read", "Write", "Edit", "Glob", "NotebookRead", "NotebookEdit", "Cd"];

/// Upstream `Zf.bashPrefixTools`: tools whose rule content may use the trailing `:*` prefix syntax.
pub(crate) const SETTINGS_PERMISSION_PREFIX_TOOLS: [&str; 1] = ["Bash"];

/// The tool name every permission rule for a Model Context Protocol tool starts with.
pub(crate) const SETTINGS_PERMISSION_MCP_PREFIX: &str = "mcp";

/// Separator between the segments of a Model Context Protocol tool name.
pub(crate) const SETTINGS_PERMISSION_MCP_SEPARATOR: &str = "__";

/// Rule content that makes a Bash prefix rule empty.
pub(crate) const SETTINGS_PERMISSION_EMPTY_PREFIX: &str = ":*";

/// The `WebSearch` characters upstream refuses in rule content.
pub(crate) const SETTINGS_PERMISSION_WEB_SEARCH_WILDCARDS: [char; 2] = ['*', '?'];

/// The `WebFetch` rule content prefix, and the marker of a URL upstream refuses.
pub(crate) const SETTINGS_PERMISSION_WEB_FETCH_DOMAIN_PREFIX: &str = "domain:";
pub(crate) const SETTINGS_PERMISSION_WEB_FETCH_URL_MARKER: &str = "://";
pub(crate) const SETTINGS_PERMISSION_WEB_FETCH_HTTP_PREFIX: &str = "http";

/// ASCII characters as the UTF-16 code units the JavaScript string ports walk.
pub(crate) const UTF16_BACKSLASH: u16 = b'\\' as u16;
pub(crate) const UTF16_BANG: u16 = b'!' as u16;
pub(crate) const UTF16_CARET: u16 = b'^' as u16;
pub(crate) const UTF16_CLOSE_BRACE: u16 = b'}' as u16;
pub(crate) const UTF16_CLOSE_BRACKET: u16 = b']' as u16;
pub(crate) const UTF16_CLOSE_PAREN: u16 = b')' as u16;
pub(crate) const UTF16_COLON: u16 = b':' as u16;
pub(crate) const UTF16_COMMA: u16 = b',' as u16;
pub(crate) const UTF16_DASH: u16 = b'-' as u16;
pub(crate) const UTF16_DOT: u16 = b'.' as u16;
pub(crate) const UTF16_EQUALS: u16 = b'=' as u16;
pub(crate) const UTF16_GREATER: u16 = b'>' as u16;
pub(crate) const UTF16_LESS: u16 = b'<' as u16;
pub(crate) const UTF16_OPEN_BRACE: u16 = b'{' as u16;
pub(crate) const UTF16_OPEN_BRACKET: u16 = b'[' as u16;
pub(crate) const UTF16_OPEN_PAREN: u16 = b'(' as u16;
pub(crate) const UTF16_PIPE: u16 = b'|' as u16;
pub(crate) const UTF16_QUESTION: u16 = b'?' as u16;
pub(crate) const UTF16_SLASH: u16 = b'/' as u16;

/// The largest repetition count the JavaScript regular expression engine keeps; larger counts clamp to it.
pub(crate) const SETTINGS_REGEXP_REPEAT_LIMIT: u64 = i32::MAX as u64;

/// The variable-name slots of one AWS credential export pair.
pub(crate) const SETTINGS_AWS_PAIR_SLOTS: [&str; 3] = ["accessKeyIdVar", "secretAccessKeyVar", "sessionTokenVar"];

/// Settings arrays the TypeScript sandbox merge concatenates and de-duplicates (`$et`).
pub(crate) const SANDBOX_MERGED_LISTS: [&str; 5] = [
    "filesystem.denyRead",
    "filesystem.denyWrite",
    "network.deniedDomains",
    "credentials.files",
    "credentials.envVars",
];

/// Settings objects the TypeScript sandbox merge replaces whole instead of merging (`zet`).
pub(crate) const SANDBOX_REPLACED_OBJECTS: [&str; 2] = ["ripgrep", "network.tlsTerminate"];

/// Keys the TypeScript sandbox merge never copies (`Bet`).
pub(crate) const SANDBOX_FORBIDDEN_KEYS: [&str; 3] = ["__proto__", "constructor", "prototype"];

/// Proxy ports dropped from the merged sandbox when the new sandbox sets domain rules (`jet`).
pub(crate) const SANDBOX_PROXY_PORTS: [&str; 2] = ["httpProxyPort", "socksProxyPort"];

/// The error TypeScript raises for a settings file path combined with the `sandbox` option.
pub(crate) const SETTINGS_PATH_WITH_SANDBOX_ERROR: &str = "Cannot use both a settings file path and the sandbox option. Include the sandbox configuration in your settings file instead.";

/// Environment variable `sdk.mjs` deletes so a host cannot force agent progress summaries on.
pub(crate) const AGENT_PROGRESS_SUMMARIES_ENV: &str = "CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES";

/// Python `VERBATIM_PROMPTS_MINIMUM_CLAUDE_CODE_VERSION`: the first CLI that honors `client_composed`.
pub(crate) const VERBATIM_PROMPTS_MINIMUM: [u32; 3] = [2, 1, 248];

/// Value of `managedSourcesBehavior` that merges the managed tiers instead of letting the first win.
pub(crate) const MANAGED_SOURCES_MERGE: &str = "merge";

/// Keys whose arrays a managed tier replaces whole instead of unioning (`UX` upstream).
pub(crate) const REPLACED_ADMIN_ARRAY_KEYS: [&str; 5] = [
    "allowedMcpServers",
    "availableModels",
    "strictKnownMarketplaces",
    "allowedChannelPlugins",
    "allowedProviders",
];

/// Keys a lower managed tier also replaces whole, but the top tier unions (`AQe` upstream).
pub(crate) const LOWER_TIER_REPLACED_KEYS: [&str; 2] = ["awsPairs", "ripgrep"];

/// Key of the sandbox AWS credential pairs the merge treats specially.
pub(crate) const AWS_PAIRS_KEY: &str = "awsPairs";

/// AWS credential variables a pair can name (`qi` upstream).
pub(crate) const AWS_CREDENTIAL_VARIABLES: [&str; 3] =
    ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN"];

/// Prefix of the suppressor variable a merged AWS pair gets (`ES` upstream).
pub(crate) const AWS_PAIR_SUPPRESSOR_PREFIX: &str = "_PARENT_PAIR_SUPPRESSOR_";

/// Fields of an AWS pair entry that name a variable (`cX` upstream).
pub(crate) const AWS_PAIR_FIELDS: [&str; 3] = ["accessKeyIdVar", "secretAccessKeyVar", "sessionTokenVar"];

/// Paths a lower managed tier never contributes (`mQe` upstream).
pub(crate) const LOWER_TIER_DROPPED_PATHS: [&[&str]; 2] = [
    &["permissions", "defaultMode"],
    &["modelPicker", "replaceBuiltInOptions"],
];

/// Keys a lower managed tier never contributes (`YX()` upstream).
pub(crate) const LOWER_TIER_STRIPPED_KEYS: [&str; 17] = [
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "gcpAuthRefresh",
    "otelHeadersHelper",
    "proxyAuthHelper",
    "forceLoginOrgUUID",
    "forceLoginMethod",
    "forceLoginGatewayUrl",
    "gatewayInternalNetworks",
    "parentSettingsBehavior",
    "env",
    "modelPicker",
    "policyHelper",
    "policyHelpers",
    "managedSourcesBehavior",
    "wslInheritsWindowsSettings",
];

/// Path of the sandbox platform list a lower managed tier is checked against.
pub(crate) const SANDBOX_PLATFORMS_PATH: [&str; 2] = ["sandbox", "enabledPlatforms"];

/// Key that unions across managed tiers in the restrictive pass.
pub(crate) const STRICT_PLUGIN_ONLY_KEY: &str = "strictPluginOnlyCustomization";

/// Key whose value one managed tier owns whole (`CQe` upstream).
pub(crate) const MODEL_OVERRIDES_KEY: &str = "modelOverrides";

/// Key that tells which managed tier owns the model overrides (`CQe` upstream).
pub(crate) const AVAILABLE_MODELS_KEY: &str = "availableModels";

/// Each restrictive rule: its settings path and the JSON values it ranks, most restrictive first (`Go()` upstream).
pub(crate) const RESTRICTIVE_RULES: [(&[&str], &[&str]); 72] = [
    (&["allowManagedPermissionRulesOnly"], &["true"]),
    (&["allowManagedHooksOnly"], &["true"]),
    (&["allowManagedMcpServersOnly"], &["true"]),
    (&["enforceAvailableModels"], &["true"]),
    (&["disableAllHooks"], &["true"]),
    (&["disableClaudeAiConnectors"], &["true"]),
    (&["disableCommandPluginSources"], &["true"]),
    (&["disableSideloadFlags"], &["true"]),
    (&["disableSkillShellExecution"], &["true"]),
    (&["disableWindowsBashCommandSplit"], &["true"]),
    (&["disableRemoteControl"], &["true"]),
    (&["disableAgentView"], &["true"]),
    (&["disableWorkflows"], &["true"]),
    (&["disableArtifact"], &["true"]),
    (&["disableBundledSkills"], &["true"]),
    (&["fastModePerSessionOptIn"], &["true"]),
    (&["isolatePeerMachines"], &["true"]),
    (&["strictPluginOnlyCustomization"], &["true"]),
    (&["disableAutoMode"], &["\"disable\""]),
    (&["disableDeepLinkRegistration"], &["\"disable\""]),
    (&["permissions", "disableBypassPermissionsMode"], &["\"disable\""]),
    (&["permissions", "disableAutoMode"], &["\"disable\""]),
    (&["permissions", "blockReadsOutsideWorkingDirectories"], &["true"]),
    (&["permissions", "blockWritesOutsideWorkingDirectories"], &["true"]),
    (&["permissions", "denyInsteadOfPrompting"], &["true"]),
    (&["autoMode", "classifyAllShell"], &["true"]),
    (&["worktree", "bgIsolation"], &["\"worktree\""]),
    (&["enableArtifact"], &["false"]),
    (&["enableWorkflows"], &["false"]),
    (&["syncClaudeAiSkills"], &["false"]),
    (&["syncClaudeAiPlugins"], &["false"]),
    (&["useAutoModeDuringPlan"], &["false"]),
    (&["skipDangerousModePermissionPrompt"], &["false"]),
    (&["skipAutoPermissionPrompt"], &["false"]),
    (&["enableAllProjectMcpServers"], &["false"]),
    (&["channelsEnabled"], &["false"]),
    (&["skipWebFetchPreflight"], &["false"]),
    (&["skipWorkflowUsageWarning"], &["false"]),
    (&["autoUploadSessions"], &["false"]),
    (&["remoteControlAtStartup"], &["false"]),
    (&["remoteTools", "allowUnattendedServing"], &["false"]),
    (&["autoContinueAtUsageLimit"], &["false"]),
    (&["responseComparisonEnabled"], &["false"]),
    (&["attribution", "sessionUrl"], &["false"]),
    (&["crossSessionInbound"], &["\"refuse\"", "\"hold\""]),
    (&["remoteControl", "shareHostProfile"], &["\"off\"", "\"basic\""]),
    (&["modelProposedGoals"], &["\"disabled\"", "\"alwaysAsk\""]),
    (
        &["maxEffortLevel"],
        &["\"low\"", "\"medium\"", "\"high\"", "\"xhigh\"", "\"max\""],
    ),
    (&["feedbackDrafts"], &["\"off\""]),
    (&["availableModelsMatch"], &["\"exact\""]),
    (&["askUserQuestionTimeout"], &["\"never\""]),
    (&["dialogExpiry"], &["\"never\""]),
    (&["sandbox", "enabled"], &["true"]),
    (&["sandbox", "failIfUnavailable"], &["true"]),
    (&["sandbox", "autoAllowBashIfSandboxed"], &["false"]),
    (&["sandbox", "allowUnsandboxedCommands"], &["false"]),
    (&["sandbox", "enableWeakerNestedSandbox"], &["false"]),
    (&["sandbox", "enableWeakerNetworkIsolation"], &["false"]),
    (&["sandbox", "allowAppleEvents"], &["false"]),
    (&["sandbox", "allowNestedUserNamespaces"], &["false"]),
    (&["sandbox", "network", "allowManagedDomainsOnly"], &["true"]),
    (&["sandbox", "network", "strictAllowlist"], &["true"]),
    (&["sandbox", "network", "allowAllUnixSockets"], &["false"]),
    (&["sandbox", "network", "allowLocalBinding"], &["false"]),
    (&["sandbox", "filesystem", "allowManagedReadPathsOnly"], &["true"]),
    (&["sandbox", "filesystem", "disabled"], &["false"]),
    (&["sandbox", "credentials", "allowPlaintextInject"], &["false"]),
    (&["sandbox", "credentials", "sigv4", "streaming"], &["\"deny\""]),
    (&["sandbox", "credentials", "sigv4", "presigned"], &["\"deny\""]),
    (&["sandbox", "credentials", "sigv4", "sigv4a"], &["\"deny\""]),
    (&["isolation", "required"], &["true"]),
    (&["isolation", "persistHome"], &["false"]),
];

/// The ranked value of a restrictive rule whose restrictive value is `true` itself (`restrictive: true` in `Go()`).
pub(crate) const RESTRICTIVE_TRUE: &str = "true";

/// Keys the restriction of a user-writable policy file never keeps from the file as written (`yJ` upstream).
pub(crate) const RESTRICT_SKIPPED_KEYS: [&str; 2] = ["disableAllHooks", "disableReplyCheck"];

/// Path the restriction pins to `false` when a policy file turns attribution off.
pub(crate) const RESTRICT_ATTRIBUTION_PATH: [&str; 2] = ["attribution", "sessionUrl"];

/// Permission keys a restricted policy file keeps.
pub(crate) const RESTRICT_PERMISSION_KEYS: [&str; 4] =
    ["deny", "ask", "disableBypassPermissionsMode", "disableAutoMode"];

/// Keys a restricted policy file keeps as written (`cXe` upstream).
pub(crate) const RESTRICT_COPIED_KEYS: [&str; 15] = [
    "allowedMcpServers",
    "deniedMcpServers",
    "allowManagedMcpServersOnly",
    "disabledMcpjsonServers",
    "allowManagedHooksOnly",
    "allowedHttpHookUrls",
    "strictKnownMarketplaces",
    "allowedMarketplaces",
    "blockedMarketplaces",
    "strictPluginOnlyCustomization",
    "availableModels",
    "enforceAvailableModels",
    "availableModelsMatch",
    "deniedModels",
    "allowedProviders",
];

/// Sandbox filesystem keys a restricted policy file keeps.
pub(crate) const RESTRICT_FILESYSTEM_KEYS: [&str; 3] = ["denyRead", "denyWrite", "allowManagedReadPathsOnly"];

/// Sandbox network keys a restricted policy file keeps.
pub(crate) const RESTRICT_NETWORK_KEYS: [&str; 3] = ["deniedDomains", "strictAllowlist", "allowManagedDomainsOnly"];

/// Sandbox credential keys a restricted policy file keeps.
pub(crate) const RESTRICT_CREDENTIAL_KEYS: [&str; 2] = ["files", "envVars"];

/// Paths the restriction always drops (`p8e` upstream).
pub(crate) const RESTRICT_DROPPED_PATHS: [&[&str]; 3] = [
    &["allowedHttpHookUrls"],
    &["httpHookAllowedEnvVars"],
    &["isolation", "required"],
];

/// Credential lists the restriction rewrites into deny entries, with the field each entry names (`p5` upstream).
pub(crate) const RESTRICT_DENY_ENTRY_LISTS: [(&[&str], &str); 2] = [
    (&["sandbox", "credentials", "files"], "path"),
    (&["sandbox", "credentials", "envVars"], "name"),
];

/// Permission lists the restriction drops negated path rules from.
pub(crate) const RESTRICT_PERMISSION_LISTS: [&[&str]; 2] = [&["permissions", "deny"], &["permissions", "ask"]];

/// Rule prefixes whose negated path form (`!path` or `./!path`) the restriction drops (`If` upstream).
pub(crate) const RESTRICT_NEGATABLE_RULE_PREFIXES: [&str; 2] = ["Read(", "Edit("];

/// Plugin the organization policy-mod options belong to (`hu` upstream).
pub(crate) const SECURITY_PLUGIN_ID: &str = "cc-plugin-sec-default@builtin";

/// Earlier names of the security plugin whose options still count, read before the current one (`N4` upstream).
pub(crate) const LEGACY_SECURITY_PLUGIN_IDS: [&str; 1] = ["sec-default@builtin"];

/// Security plugin options a restricted policy file keeps (`qy.policyModOptions` upstream).
pub(crate) const POLICY_MOD_OPTIONS: [&str; 1] = ["allowManagedModsOnly"];

/// Marketplace of the plugins that ship with the CLI (`YS` upstream).
pub(crate) const BUILTIN_PLUGIN_MARKETPLACE: &str = "builtin";

/// Settings key that pins the organization a login must belong to.
pub(crate) const LOGIN_ORGANIZATION_KEY: &str = "forceLoginOrgUUID";

/// Settings key that pins the login method.
pub(crate) const LOGIN_METHOD_KEY: &str = "forceLoginMethod";

/// Login methods the machine policy can pin.
pub(crate) const LOGIN_METHODS: [&str; 2] = ["claudeai", "console"];

/// Folder name whose contents the connector download flow owns (`J8e` upstream), compared after normalization.
pub(crate) const CONNECTOR_DOWNLOADS_NAME: &str = "connector-downloads";

/// Whole contents of a `.gitignore` that marks a connector download folder as ignored (`n7e` upstream).
pub(crate) const CONNECTOR_GITIGNORE_MARKERS: [&[u8]; 3] = [b"*", b"*\n", b"*\r\n"];

/// Largest `.gitignore` the connector check reads (`t7e` upstream).
pub(crate) const CONNECTOR_GITIGNORE_MAX_BYTES: u64 = 3;

/// Entries the connector folder check remembers before it forgets all of them (`nE` upstream).
pub(crate) const CONNECTOR_CACHE_LIMIT: usize = 64;

/// Symbolic links the path walk follows before it gives up on the path.
pub(crate) const LINK_HOP_LIMIT: usize = 64;

/// The sandbox mount that always counts as a connector download folder (`z8e` upstream, the parent of
/// `/mnt/user-data/uploads` joined with `/connector-downloads`).
pub(crate) const CONNECTOR_DOWNLOADS_MOUNT: &str = "/mnt/user-data/connector-downloads";

/// Bytes of a `.gitignore` the connector check reads (`t7e` upstream).
pub(crate) const CONNECTOR_GITIGNORE_READ_BYTES: usize = 4;

/// Error code the path vetting walk throws when it refuses a path (`yd` upstream).
pub(crate) const VETTING_REFUSAL_CODE: &str = "ERR_SYMLINK_TO_NETWORK_PATH";

/// Environment variable naming the kind of session the CLI runs as (`Nm` upstream).
pub(crate) const VETTING_SESSION_KIND_VARIABLE: &str = "CLAUDE_CODE_SESSION_KIND";

/// Session kind whose launch directory the path vetting walk does not trust (`Nm` upstream).
pub(crate) const VETTING_BACKGROUND_SESSION_KIND: &str = "bg";

/// macOS folder of open file descriptors, which the walk never reads through.
pub(crate) const VETTING_DEV_FD: &str = "/dev/fd";

/// Folder the macOS automounter may own (`TI.homeIsOwnMount` upstream).
pub(crate) const VETTING_HOME_MOUNT: &str = "/home";

/// Folder whose device `/home` is compared with (`TI.homeIsOwnMount` upstream).
pub(crate) const VETTING_USERS_MOUNT: &str = "/Users";

/// libuv's `uv_translate_sys_error` as node runs it on Windows: the `code` each Windows system error takes, in the
/// order of its `switch`. A code missing here is `UNKNOWN`.
pub(crate) const LIBUV_WINDOWS_ERRORS: [(i32, &str); 100] = [
    (10013, "EACCES"),
    (740, "EACCES"),
    (1920, "EACCES"),
    (1227, "EADDRINUSE"),
    (10048, "EADDRINUSE"),
    (10049, "EADDRNOTAVAIL"),
    (10047, "EAFNOSUPPORT"),
    (10035, "EAGAIN"),
    (232, "EAGAIN"),
    (10037, "EALREADY"),
    (1004, "EBADF"),
    (6, "EBADF"),
    (33, "EBUSY"),
    (231, "EBUSY"),
    (32, "EBUSY"),
    (995, "ECANCELED"),
    (10004, "ECANCELED"),
    (1113, "ECHARSET"),
    (1236, "ECONNABORTED"),
    (10053, "ECONNABORTED"),
    (1225, "ECONNREFUSED"),
    (10061, "ECONNREFUSED"),
    (64, "ECONNRESET"),
    (10054, "ECONNRESET"),
    (183, "EEXIST"),
    (80, "EEXIST"),
    (998, "EFAULT"),
    (10014, "EFAULT"),
    (1232, "EHOSTUNREACH"),
    (10065, "EHOSTUNREACH"),
    (122, "EINVAL"),
    (13, "EINVAL"),
    (87, "EINVAL"),
    (1464, "EINVAL"),
    (10022, "EINVAL"),
    (10046, "EINVAL"),
    (1102, "EIO"),
    (1111, "EIO"),
    (23, "EIO"),
    (1166, "EIO"),
    (1165, "EIO"),
    (1393, "EIO"),
    (1129, "EIO"),
    (1101, "EIO"),
    (31, "EIO"),
    (1106, "EIO"),
    (1117, "EIO"),
    (1104, "EIO"),
    (205, "EIO"),
    (110, "EIO"),
    (1103, "EIO"),
    (156, "EIO"),
    (10056, "EISCONN"),
    (1921, "ELOOP"),
    (4, "EMFILE"),
    (10024, "EMFILE"),
    (10040, "EMSGSIZE"),
    (111, "ENAMETOOLONG"),
    (206, "ENAMETOOLONG"),
    (1231, "ENETUNREACH"),
    (10051, "ENETUNREACH"),
    (10055, "ENOBUFS"),
    (161, "ENOENT"),
    (267, "ENOENT"),
    (203, "ENOENT"),
    (2, "ENOENT"),
    (123, "ENOENT"),
    (15, "ENOENT"),
    (4392, "ENOENT"),
    (126, "ENOENT"),
    (3, "ENOENT"),
    (11001, "ENOENT"),
    (11004, "ENOENT"),
    (8, "ENOMEM"),
    (14, "ENOMEM"),
    (82, "ENOSPC"),
    (112, "ENOSPC"),
    (277, "ENOSPC"),
    (1100, "ENOSPC"),
    (39, "ENOSPC"),
    (2250, "ENOTCONN"),
    (10057, "ENOTCONN"),
    (145, "ENOTEMPTY"),
    (10038, "ENOTSOCK"),
    (50, "ENOTSUP"),
    (109, "EOF"),
    (5, "EPERM"),
    (1314, "EPERM"),
    (230, "EPIPE"),
    (233, "EPIPE"),
    (10058, "EPIPE"),
    (10043, "EPROTONOSUPPORT"),
    (19, "EROFS"),
    (121, "ETIMEDOUT"),
    (10060, "ETIMEDOUT"),
    (17, "EXDEV"),
    (1, "EISDIR"),
    (208, "E2BIG"),
    (10044, "ESOCKTNOSUPPORT"),
    (193, "EFTYPE"),
];

/// `ERROR_INVALID_HANDLE`, which libuv's Windows `realpath` raises for a final path it cannot shorten.
pub(crate) const WINDOWS_ERROR_INVALID_HANDLE: i32 = 6;

/// The prefix `GetFinalPathNameByHandleW` puts on a network path, which libuv's `realpath` turns into `\\`.
pub(crate) const LIBUV_UNC_PATH_PREFIX: &str = "\\\\?\\UNC\\";

/// The prefix `GetFinalPathNameByHandleW` puts on a local path, which libuv's `realpath` drops.
pub(crate) const LIBUV_LONG_PATH_PREFIX: &str = "\\\\?\\";

/// The UTF-16 units node reads for each byte of the WTF-8 spelling of a lone surrogate: one U+FFFD apiece.
pub(crate) const LIBUV_LONE_SURROGATE_TEXT: [u16; 3] = [0xFFFD; 3];

/// The `code` libuv gives a Windows system error its table does not name.
pub(crate) const LIBUV_UNKNOWN_ERROR: &str = "UNKNOWN";

/// The macOS automounter map that says whether `/home` is automounted (`TI.readAutoMaster` upstream).
pub(crate) const VETTING_AUTO_MASTER: &str = "/etc/auto_master";

/// Path components the automount checks look at (`NQ` upstream).
pub(crate) const VETTING_AUTOMOUNT_DEPTH: usize = 6;

/// Folders the macOS data volume firmlinks to the root (`VQ` upstream), in upstream order.
pub(crate) const VETTING_FIRMLINKED_FOLDERS: [&str; 16] = [
    "usr/local",
    "usr/libexec/cups",
    "usr/share/snmp",
    "AppleInternal",
    "Applications",
    "Library",
    "Users",
    "Volumes",
    "cores",
    "home",
    "media",
    "mnt",
    "opt",
    "pkg",
    "private",
    "sw",
];

/// Milliseconds the machine policy reading stays in force when the last read answered (`ret` upstream).
pub(crate) const MDM_REREAD_IDLE_MS: i128 = 1_800_000;

/// Milliseconds between re-reads while the machine keeps not answering (`_q` upstream).
pub(crate) const MDM_REREAD_RETRY_MS: i128 = 60_000;

/// Milliseconds before a policy change that would drop settings is believed (`set` upstream).
pub(crate) const MDM_REREAD_LOSS_MS: i128 = 30_000;

/// Milliseconds a call waits for a re-read before it goes on with the reading in force (`oet` upstream).
pub(crate) const MDM_REREAD_WAIT_MS: i128 = 1_000;

/// The longest a call waits for a background re-read by the machine's own steady clock, whatever the host's clock
/// shows, so a clock that stands still or goes back cannot hold the caller past the second upstream waits.
pub(crate) const MDM_REREAD_WAIT: Duration = Duration::from_millis(1_000);

/// Unanswered reads in a row after which the slow idle interval applies again.
pub(crate) const MDM_UNANSWERED_RETRY_LIMIT: u32 = 4;

/// Connection info fields the type names; every other `initialize` response field is kept as an additional field.
pub(crate) const CONNECTION_INFO_FIELDS: &[&str] = &[
    "commands",
    "models",
    "agents",
    "account",
    "output_style",
    "available_output_styles",
    "hooks_applied",
    "hook_on_failure_supported",
    "plugins_applied",
    "managed_settings_applied",
    "sdk_mcp_manifests_parked",
    "claude_code_version",
    "fast_mode_state",
    "fast_mode_disabled_reason",
];

/// The base-36 digits of JavaScript `Number.prototype.toString(36)`, also used for the hex of request ids.
pub(crate) const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// TypeScript: how long `initialize` waits when blocking hooks or managed settings are part of the request.
pub(crate) const TYPESCRIPT_INITIALIZE_TIMEOUT: Duration = Duration::from_millis(660_000);

/// TypeScript `qet`: the wait for the process to exit on its own after stdin closes.
pub(crate) const TYPESCRIPT_CLOSE_GRACE: Duration = Duration::from_millis(2000);

/// TypeScript: the wait between SIGTERM and SIGKILL.
pub(crate) const TYPESCRIPT_KILL_DELAY: Duration = Duration::from_millis(5000);

/// TypeScript `Yet`: how long stderr may stay open after the exit.
pub(crate) const STDERR_SETTLE: Duration = Duration::from_millis(200);

/// Python: each wait of its close sequence.
pub(crate) const PYTHON_CLOSE_WAIT: Duration = Duration::from_secs(5);

pub(crate) const OAUTH_REFRESH_ENV: &str = "CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH";
pub(crate) const HOST_AUTH_REFRESH_ENV: &str = "CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH";
pub(crate) const QUESTION_PREVIEW_FORMAT_ENV: &str = "CLAUDE_CODE_QUESTION_PREVIEW_FORMAT";
pub(crate) const QUESTION_EXTENDED_ENV: &str = "CLAUDE_CODE_QUESTION_EXTENDED";
pub(crate) const QUESTION_OPTIONAL_DESCRIPTIONS_ENV: &str = "CLAUDE_CODE_QUESTION_OPTIONAL_DESCRIPTIONS";

/// Variables upstream deletes from an inherited environment (in any letter case) unless `toolConfig` sets them.
pub(crate) const TOOL_CONFIG_ENVS: [&str; 3] = [
    QUESTION_EXTENDED_ENV,
    QUESTION_OPTIONAL_DESCRIPTIONS_ENV,
    AGENT_PROGRESS_SUMMARIES_ENV,
];

pub(crate) const OAUTH_CALLBACK_MISSING: &str = "getOAuthToken callback is not provided.";
pub(crate) const HOST_AUTH_CALLBACK_MISSING: &str = "getHostAuthToken callback is not provided.";

/// The protocol versions the Python `mcp` server negotiates; anything else is answered with the latest.
pub(crate) const PYTHON_MCP_PROTOCOL_VERSIONS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// The protocol version the Python server answers with when the client asks for an unsupported one.
pub(crate) const PYTHON_MCP_LATEST_PROTOCOL_VERSION: &str = "2025-11-25";

/// The requests a server built by `create_sdk_mcp_server` registers handlers for.
pub(crate) const PYTHON_MCP_METHODS: [&str; 4] = ["initialize", "ping", "tools/list", "tools/call"];

/// The `ToolAnnotations` fields `Tool.model_validate` keeps.
pub(crate) const PYTHON_MCP_ANNOTATION_FIELDS: [&str; 5] = [
    "title",
    "readOnlyHint",
    "destructiveHint",
    "idempotentHint",
    "openWorldHint",
];

/// `Number.MAX_SAFE_INTEGER`, the largest integer request id the TypeScript dialect accepts.
pub(crate) const JS_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// The `$schema` the TypeScript dialect adds to a tool input schema.
pub(crate) const MCP_TS_JSON_SCHEMA_DRAFT_07: &str = "http://json-schema.org/draft-07/schema#";

/// Variable naming the surface that launched the process.
pub(crate) const ENTRYPOINT_VARIABLE: &str = "CLAUDE_CODE_ENTRYPOINT";

/// Entrypoints of a cloud session that takes its managed settings from the organization (`Cf` upstream).
pub(crate) const ORGANIZATION_ENTRYPOINTS: [&str; 2] = ["remote_cowork", "remote_cowork_trigger"];

/// Allow-list keys a document the organization withholds can still narrow (`JP` upstream).
pub(crate) const HELD_ALLOW_LIST_KEYS: [&str; 8] = [
    "allowedMcpServers",
    "availableModels",
    "strictKnownMarketplaces",
    "allowedChannelPlugins",
    "allowedProviders",
    "allowedMarketplaces",
    "allowedHttpHookUrls",
    "httpHookAllowedEnvVars",
];

/// Gate keys that mean nothing without the allow list they gate (`HX` upstream).
pub(crate) const ALLOW_LIST_GATES: [(&str, &str); 2] = [
    ("allowManagedMcpServersOnly", "allowedMcpServers"),
    ("enforceAvailableModels", "availableModels"),
];

/// The allow list of MCP servers, which the admin may take from any managed tier when servers are managed-only.
pub(crate) const MCP_SERVER_ALLOW_LIST_KEY: &str = "allowedMcpServers";

/// Key that says whether the parent process's managed settings merge under the managed tier.
pub(crate) const PARENT_SETTINGS_BEHAVIOR_KEY: &str = "parentSettingsBehavior";

/// Links the writability walk follows before it gives up and treats the document as writable (`DYe` upstream).
pub(crate) const WRITABLE_WALK_LINK_LIMIT: usize = 40;

/// The write bit for others (`S_IWOTH`) and for the group (`S_IWGRP`) of a mode.
pub(crate) const MODE_OTHER_WRITABLE: u32 = 0o2;
pub(crate) const MODE_GROUP_WRITABLE: u32 = 0o20;

// Node `path` port

/// UTF-16 code unit of `.`.
pub(crate) const NODE_PATH_CHAR_DOT: u16 = 0x2e;

/// UTF-16 code unit of `/`.
pub(crate) const NODE_PATH_CHAR_FORWARD_SLASH: u16 = 0x2f;

/// UTF-16 code unit of `\`.
pub(crate) const NODE_PATH_CHAR_BACKWARD_SLASH: u16 = 0x5c;

/// UTF-16 code unit of `:`.
pub(crate) const NODE_PATH_CHAR_COLON: u16 = 0x3a;

/// Device names the Windows flavor of node's `path` treats as reserved (compared after `toUpperCase`).
pub(crate) const NODE_WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// TypeScript subtypes addressed to the machine serving the session's tools, which the SDK leaves unanswered.
pub(crate) const REMOTE_TOOL_SUBTYPES: [&str; 4] = [
    "remote_tool_call",
    "remote_plumbing_call",
    "remote_tools_probe",
    "remote_tools_reannounce",
];

/// The most nested arrays and objects `TypedMcpToolHandler` reads tool arguments through, the recursion limit
/// `serde_json` reads JSON text with.
pub(crate) const TYPED_TOOL_ARGUMENTS_MAX_DEPTH: usize = 128;

/// The longest tool name the TypeScript `McpServer` registers without printing a warning.
pub(crate) const MCP_TS_TOOL_NAME_MAX_LENGTH: usize = 128;

/// Python 3.11+ refuses to convert an integer of more than this many digits.
pub(crate) const PYTHON_INTEGER_DIGIT_LIMIT: usize = 4300;

/// Nesting beyond this is refused instead of recursing without bound when a request id is scanned.
pub(crate) const RAW_JSON_MAX_DEPTH: usize = 200;

/// The prefix of the placeholder text a request id that a `Value` cannot hold stands for until it is written.
pub(crate) const PYTHON_ID_MARKER_PREFIX: &str = "\u{0}pyid:";

/// TypeScript `oA`: how long the WebSocket transport's connection may take to open.
pub(crate) const WEBSOCKET_CONNECT_TIMEOUT: Duration = Duration::from_millis(15_000);

/// TypeScript `g4`: the delay between the WebSocket transport's keep-alive frames.
pub(crate) const WEBSOCKET_KEEP_ALIVE_INTERVAL: Duration = Duration::from_millis(50_000);

/// The line the WebSocket transport sends as a keep-alive frame.
pub(crate) const WEBSOCKET_KEEP_ALIVE_LINE: &str = "{\"type\":\"keep_alive\"}\n";

/// The reason the WebSocket transport gives when it closes the connection.
pub(crate) const WEBSOCKET_CLOSE_REASON: &str = "Normal closure";

/// The code a browser reports for a close frame without a status.
pub(crate) const WEBSOCKET_NO_STATUS_CODE: u16 = 1005;

/// The code a browser reports when the connection drops without a close frame.
pub(crate) const WEBSOCKET_ABNORMAL_CODE: u16 = 1006;

/// The pydantic model union a JSON-RPC message is validated against, as pydantic titles its `ValidationError`.
pub(crate) const PYDANTIC_JSONRPC_MESSAGE_TITLE: &str =
    "union[JSONRPCRequest,JSONRPCNotification,JSONRPCResponse,JSONRPCError]";

/// pydantic refuses to convert a string of more than this many significant digits to an integer.
pub(crate) const PYDANTIC_MAX_INT_DIGITS: usize = 4300;

/// The pydantic minor version that documents each error type.
pub(crate) const PYDANTIC_DOCUMENTATION_VERSION: &str = "2.14";

/// pydantic-core shows at most this many bytes of an input's `repr`.
pub(crate) const PYDANTIC_MAX_REPR_BYTES: usize = 50;

/// The bytes pydantic-core keeps from the start of a `repr` it truncates.
pub(crate) const PYDANTIC_REPR_HEAD_BYTES: usize = 25;

/// The bytes pydantic-core keeps from the end of a `repr` it truncates.
pub(crate) const PYDANTIC_REPR_TAIL_BYTES: usize = 24;

/// `OE`: the prefix of every request id the manifest capture sends, which marks a late reply for dropping.
pub(crate) const MCP_MANIFEST_CAPTURE_ID_PREFIX: &str = "sdk-manifest-capture:";

/// `$8`: how long the manifest capture waits for every server before the CLI is left to handshake the rest.
pub(crate) const MCP_MANIFEST_CAPTURE_DEADLINE: Duration = Duration::from_millis(250);

/// `F8`: the protocol version the manifest capture's `initialize` asks for.
pub(crate) const MCP_MANIFEST_CAPTURE_PROTOCOL_VERSION: &str = "2025-11-25";

/// Python's precision in `invalid literal for int() with base 10: %.200R`.
pub(crate) const PYTHON_INT_LITERAL_REPR_CHARS: usize = 200;

/// Python's floor and default for the initialize wait, in seconds: `max(int(...) / 1000.0, 60.0)`.
pub(crate) const PYTHON_INITIALIZE_TIMEOUT_SECONDS: f64 = 60.0;

/// The `uiHost` subtypes the driver routes to `UiHost`.
pub(crate) const UI_SUBTYPES: [&str; 6] = [
    "ui_copy",
    "ui_prompt_read",
    "ui_prompt_fill",
    "ui_prompt_paint",
    "ui_prompt_suggest",
    "ui_read_selection",
];

/// `AE`: the startup initialize limit when the caller passes none.
pub(crate) const STARTUP_DEFAULT_LIMIT: Duration = Duration::from_millis(60_000);
/// `w8`: how long hook time stays uncounted, only when the caller passed no limit.
pub(crate) const STARTUP_UNCOUNTED_BUDGET: Duration = Duration::from_millis(600_000);
/// `itt`: how many hook ids are followed one by one; later ones are only counted.
pub(crate) const STARTUP_FOLLOWED_LIMIT: usize = 64;
/// The hook events whose running time `startup()` leaves uncounted.
pub(crate) const STARTUP_HOOK_EVENTS: [&str; 2] = ["SessionStart", "Setup"];

/// The keys of `ClaimOptions.settings` that govern hooks. A claim cannot apply them.
pub(crate) const CLAIM_HOOK_SETTINGS: [&str; 5] = [
    "hooks",
    "disableAllHooks",
    "allowManagedHooksOnly",
    "allowedHttpHookUrls",
    "httpHookAllowedEnvVars",
];

/// The settings keys that follow the claim, in the order `claim()` sends them.
pub(crate) const CLAIM_FOLLOWING_SETTINGS: [&str; 6] = [
    "model",
    "agent",
    "fastMode",
    "permissions",
    "availableModels",
    "disableAutoMode",
];

/// The session id Python's `ClaudeSDKClient.connect(str)` and `query` write when the caller names none.
pub(crate) const PYTHON_DEFAULT_SESSION: &str = "default";

/// The keyword a schema `pattern` takes in the TypeScript call check.
pub(crate) const ECMA_PATTERN: &str = "x-trogon-pattern";

/// The key TypeScript spreads after a `canUseTool` result to carry the request's `tool_use_id`.
pub(crate) const PERMISSION_TOOL_USE_ID_FIELD: &str = "toolUseID";

/// Python's `TypeError` for a `can_use_tool` callback that returned anything but a `PermissionResult`; the type
/// of the value follows.
pub(crate) const PYTHON_PERMISSION_RESULT_TYPE_ERROR: &str =
    "Tool permission callback must return PermissionResult (PermissionResultAllow or PermissionResultDeny), got ";

/// How Python names the type of a `None` result.
pub(crate) const PYTHON_NONE_TYPE: &str = "<class 'NoneType'>";

/// How Python names the type of a `dict` result.
pub(crate) const PYTHON_DICT_TYPE: &str = "<class 'dict'>";

/// Stack sizes to try for the validation thread, largest first, so deep schemas reach the frame limit before the
/// native stack runs out.
pub(crate) const STACK_SIZES: [usize; 3] = [512 << 20, 64 << 20, 8 << 20];

/// The documents `jsonschema_specifications` ships, which `jsonschema` registers under their own ids.
pub(crate) const METASCHEMAS: [&str; 20] = [
    include_str!("upstream/metaschemas/draft3/metaschema.json"),
    include_str!("upstream/metaschemas/draft4/metaschema.json"),
    include_str!("upstream/metaschemas/draft6/metaschema.json"),
    include_str!("upstream/metaschemas/draft7/metaschema.json"),
    include_str!("upstream/metaschemas/draft201909/metaschema.json"),
    include_str!("upstream/metaschemas/draft201909/vocabularies/applicator"),
    include_str!("upstream/metaschemas/draft201909/vocabularies/content"),
    include_str!("upstream/metaschemas/draft201909/vocabularies/core"),
    include_str!("upstream/metaschemas/draft201909/vocabularies/format"),
    include_str!("upstream/metaschemas/draft201909/vocabularies/meta-data"),
    include_str!("upstream/metaschemas/draft201909/vocabularies/validation"),
    include_str!("upstream/metaschemas/draft202012/metaschema.json"),
    include_str!("upstream/metaschemas/draft202012/vocabularies/applicator"),
    include_str!("upstream/metaschemas/draft202012/vocabularies/content"),
    include_str!("upstream/metaschemas/draft202012/vocabularies/core"),
    include_str!("upstream/metaschemas/draft202012/vocabularies/format-annotation"),
    include_str!("upstream/metaschemas/draft202012/vocabularies/format-assertion"),
    include_str!("upstream/metaschemas/draft202012/vocabularies/meta-data"),
    include_str!("upstream/metaschemas/draft202012/vocabularies/unevaluated"),
    include_str!("upstream/metaschemas/draft202012/vocabularies/validation"),
];

/// Used by the python_jsonschema_referencing port.
pub(crate) const NO_SUCH_ANCHOR: &str = "NoSuchAnchor";

/// Used by the python_urllib port.
pub(crate) const USES_RELATIVE: &[&str] = &[
    "", "ftp", "http", "gopher", "nntp", "imap", "wais", "file", "https", "shttp", "mms", "prospero", "rtsp", "rtsps",
    "rtspu", "sftp", "svn", "svn+ssh", "ws", "wss",
];

/// Used by the python_urllib port.
pub(crate) const USES_NETLOC: &[&str] = &[
    "",
    "ftp",
    "http",
    "gopher",
    "nntp",
    "telnet",
    "imap",
    "wais",
    "file",
    "mms",
    "https",
    "shttp",
    "snews",
    "prospero",
    "rtsp",
    "rtsps",
    "rtspu",
    "rsync",
    "svn",
    "svn+ssh",
    "sftp",
    "nfs",
    "git",
    "git+ssh",
    "ws",
    "wss",
    "itms-services",
];

/// Used by the python_urllib port.
pub(crate) const USES_PARAMS: &[&str] = &[
    "", "ftp", "hdl", "prospero", "http", "imap", "https", "shttp", "rtsp", "rtsps", "rtspu", "sip", "sips", "mms",
    "sftp", "tel",
];

/// Used by the python_re port.
pub(crate) const OP_FAILURE: u32 = 0;

/// Used by the python_re port.
pub(crate) const OP_SUCCESS: u32 = 1;

/// Used by the python_re port.
pub(crate) const OP_ANY: u32 = 2;

/// Used by the python_re port.
pub(crate) const OP_ANY_ALL: u32 = 3;

/// Used by the python_re port.
pub(crate) const OP_ASSERT: u32 = 4;

/// Used by the python_re port.
pub(crate) const OP_ASSERT_NOT: u32 = 5;

/// Used by the python_re port.
pub(crate) const OP_AT: u32 = 6;

/// Used by the python_re port.
pub(crate) const OP_BRANCH: u32 = 7;

/// Used by the python_re port.
pub(crate) const OP_CATEGORY: u32 = 8;

/// Used by the python_re port.
pub(crate) const OP_CHARSET: u32 = 9;

/// Used by the python_re port.
pub(crate) const OP_BIGCHARSET: u32 = 10;

/// Used by the python_re port.
pub(crate) const OP_GROUPREF: u32 = 11;

/// Used by the python_re port.
pub(crate) const OP_GROUPREF_EXISTS: u32 = 12;

/// Used by the python_re port.
pub(crate) const OP_IN: u32 = 13;

/// Used by the python_re port.
pub(crate) const OP_JUMP: u32 = 15;

/// Used by the python_re port.
pub(crate) const OP_LITERAL: u32 = 16;

/// Used by the python_re port.
pub(crate) const OP_MARK: u32 = 17;

/// Used by the python_re port.
pub(crate) const OP_MAX_UNTIL: u32 = 18;

/// Used by the python_re port.
pub(crate) const OP_MIN_UNTIL: u32 = 19;

/// Used by the python_re port.
pub(crate) const OP_NOT_LITERAL: u32 = 20;

/// Used by the python_re port.
pub(crate) const OP_NEGATE: u32 = 21;

/// Used by the python_re port.
pub(crate) const OP_RANGE: u32 = 22;

/// Used by the python_re port.
pub(crate) const OP_REPEAT: u32 = 23;

/// Used by the python_re port.
pub(crate) const OP_REPEAT_ONE: u32 = 24;

/// Used by the python_re port.
pub(crate) const OP_MIN_REPEAT_ONE: u32 = 26;

/// Used by the python_re port.
pub(crate) const OP_ATOMIC_GROUP: u32 = 27;

/// Used by the python_re port.
pub(crate) const OP_POSSESSIVE_REPEAT: u32 = 28;

/// Used by the python_re port.
pub(crate) const OP_POSSESSIVE_REPEAT_ONE: u32 = 29;

/// Used by the python_re port.
pub(crate) const OP_GROUPREF_IGNORE: u32 = 30;

/// Used by the python_re port.
pub(crate) const OP_IN_IGNORE: u32 = 31;

/// Used by the python_re port.
pub(crate) const OP_LITERAL_IGNORE: u32 = 32;

/// Used by the python_re port.
pub(crate) const OP_NOT_LITERAL_IGNORE: u32 = 33;

/// Used by the python_re port.
pub(crate) const OP_GROUPREF_UNI_IGNORE: u32 = 38;

/// Used by the python_re port.
pub(crate) const OP_IN_UNI_IGNORE: u32 = 39;

/// Used by the python_re port.
pub(crate) const OP_LITERAL_UNI_IGNORE: u32 = 40;

/// Used by the python_re port.
pub(crate) const OP_NOT_LITERAL_UNI_IGNORE: u32 = 41;

/// Used by the python_re port.
pub(crate) const OP_RANGE_UNI_IGNORE: u32 = 42;

/// Used by the python_re port.
pub(crate) const AT_BEGINNING: u32 = 0;

/// Used by the python_re port.
pub(crate) const AT_BEGINNING_LINE: u32 = 1;

/// Used by the python_re port.
pub(crate) const AT_BEGINNING_STRING: u32 = 2;

/// Used by the python_re port.
pub(crate) const AT_BOUNDARY: u32 = 3;

/// Used by the python_re port.
pub(crate) const AT_NON_BOUNDARY: u32 = 4;

/// Used by the python_re port.
pub(crate) const AT_END: u32 = 5;

/// Used by the python_re port.
pub(crate) const AT_END_LINE: u32 = 6;

/// Used by the python_re port.
pub(crate) const AT_END_STRING: u32 = 7;

/// Used by the python_re port.
pub(crate) const AT_UNI_BOUNDARY: u32 = 10;

/// Used by the python_re port.
pub(crate) const AT_UNI_NON_BOUNDARY: u32 = 11;

/// `CATEGORY_*` base codes; `CH_UNICODE` maps each to the code ten above it.
pub(crate) const CATEGORY_DIGIT: u32 = 0;

/// Used by the python_re port.
pub(crate) const CATEGORY_NOT_DIGIT: u32 = 1;

/// Used by the python_re port.
pub(crate) const CATEGORY_SPACE: u32 = 2;

/// Used by the python_re port.
pub(crate) const CATEGORY_NOT_SPACE: u32 = 3;

/// Used by the python_re port.
pub(crate) const CATEGORY_WORD: u32 = 4;

/// Used by the python_re port.
pub(crate) const CATEGORY_NOT_WORD: u32 = 5;

/// Used by the python_re port.
pub(crate) const CATEGORY_UNICODE_OFFSET: u32 = 10;

/// Used by the python_re port.
pub(crate) const FLAG_TEMPLATE: u32 = 1;

/// Used by the python_re port.
pub(crate) const FLAG_IGNORECASE: u32 = 2;

/// Used by the python_re port.
pub(crate) const FLAG_LOCALE: u32 = 4;

/// Used by the python_re port.
pub(crate) const FLAG_MULTILINE: u32 = 8;

/// Used by the python_re port.
pub(crate) const FLAG_DOTALL: u32 = 16;

/// Used by the python_re port.
pub(crate) const FLAG_UNICODE: u32 = 32;

/// Used by the python_re port.
pub(crate) const FLAG_VERBOSE: u32 = 64;

/// Used by the python_re port.
pub(crate) const FLAG_DEBUG: u32 = 128;

/// Used by the python_re port.
pub(crate) const FLAG_ASCII: u32 = 256;

/// Used by the python_re port.
pub(crate) const TYPE_FLAGS: u32 = FLAG_ASCII | FLAG_LOCALE | FLAG_UNICODE;

/// Used by the python_re port.
pub(crate) const GLOBAL_FLAGS: u32 = FLAG_DEBUG | FLAG_TEMPLATE;

/// `_sre.MAXGROUPS`.
pub(crate) const MAXGROUPS: u64 = 0x3fff_ffff;

/// `_sre.MAXREPEAT` (`SRE_MAXREPEAT`, a 32-bit `SRE_CODE`).
pub(crate) const MAXREPEAT: u32 = u32::MAX;

/// `_compiler.MAXCODE`.
pub(crate) const MAXCODE: u128 = u32::MAX as u128;

/// `_parser.MAXWIDTH`.
pub(crate) const MAXWIDTH: u128 = 1 << 64;

/// Used by the python_re port.
pub(crate) const RECURSION_ERROR: &str = "maximum recursion depth exceeded";

/// Used by the python_re port.
pub(crate) const SPECIAL_CHARS: &str = ".\\[{()*+?^$|";

/// Used by the python_re port.
pub(crate) const HANGUL_BASE: u32 = 0xAC00;

/// Used by the python_re port.
pub(crate) const HANGUL_LEADS: &[&str] = &[
    "G", "GG", "N", "D", "DD", "R", "M", "B", "BB", "S", "SS", "", "J", "JJ", "C", "K", "T", "P", "H",
];

/// Used by the python_re port.
pub(crate) const HANGUL_VOWELS: &[&str] = &[
    "A", "AE", "YA", "YAE", "EO", "E", "YEO", "YE", "O", "WA", "WAE", "OE", "YO", "U", "WEO", "WE", "WI", "YU", "EU",
    "YI", "I",
];

/// Used by the python_re port.
pub(crate) const HANGUL_TRAILS: &[&str] = &[
    "", "G", "GG", "GS", "N", "NJ", "NH", "D", "L", "LG", "LM", "LB", "LS", "LT", "LP", "LH", "M", "B", "BS", "S",
    "SS", "NG", "J", "C", "K", "T", "P", "H",
];

/// Used by the python_re port.
pub(crate) const HANGUL_PREFIX: &str = "HANGUL SYLLABLE ";

/// Used by the python_re port.
pub(crate) const CJK_PREFIX: &str = "CJK UNIFIED IDEOGRAPH-";

/// Used by the python_sort port.
pub(crate) const MIN_GALLOP: usize = 7;

/// CPython 3.12's `Py_C_RECURSION_LIMIT`: a C recursion check that would put more checks than this under way
/// raises `RecursionError`, whatever the frame limit. The C `repr` and rich comparison of nested containers check
/// once a level.
pub(crate) const C_RECURSION_LIMIT: usize = 10_000;

/// The C recursion checks under way when `check_schema` iterates the metaschema's errors, measured from the SDK's
/// tool handler.
pub(crate) const CHECK_SCHEMA_HELD: usize = 6;

/// The C recursion checks of resuming a generator from C, which enters a new evaluation loop: `list(...)`,
/// `next(...)`, `any(...)`, `list += ...`, or a generic `FOR_ITER`.
pub(crate) const RESUMED: usize = 2;

/// The C recursion checks of calling a builtin that checks before it runs: `any` and `all`, or a generic `CALL`.
pub(crate) const BUILTIN_CALL: usize = 1;

/// The C recursion checks under way when `best_match` iterates the errors: [`CHECK_SCHEMA_HELD`], the `max` call
/// and resuming the error generator from C.
pub(crate) const VALIDATE_HELD: usize = CHECK_SCHEMA_HELD + BUILTIN_CALL + RESUMED;

/// The frames above the caller of `isinstance(value, abc)` reaches the first time a type meets an ABC:
/// `__instancecheck__`, `_abc_instancecheck`, `__subclasscheck__`, `_abc_subclasscheck` and the subclass hooks.
pub(crate) const COLD_ABC_FRAMES: usize = 6;

/// Used by the python_jsonschema_drafts port.
pub(crate) const JSONSCHEMA_DRAFTS_DRAFT7_IN_VALUE: &[&str] = &[
    "if",
    "not",
    "else",
    "then",
    "contains",
    "propertyNames",
    "additionalItems",
    "additionalProperties",
];

/// Used by the python_jsonschema_drafts port.
pub(crate) const JSONSCHEMA_DRAFTS_DRAFT201909_IN_VALUE: &[&str] = &[
    "if",
    "not",
    "else",
    "then",
    "contains",
    "contentSchema",
    "propertyNames",
    "additionalItems",
    "unevaluatedItems",
    "additionalProperties",
    "unevaluatedProperties",
];

/// Used by the python_jsonschema_drafts port.
pub(crate) const JSONSCHEMA_DRAFTS_DRAFT202012_IN_VALUE: &[&str] = &[
    "if",
    "not",
    "else",
    "then",
    "items",
    "contains",
    "contentSchema",
    "propertyNames",
    "unevaluatedItems",
    "additionalProperties",
    "unevaluatedProperties",
];

/// `str()` of the `RecursionError` a rich comparison raises past the C recursion checks.
pub(crate) const COMPARE_RECURSION_ERROR: &str = "maximum recursion depth exceeded in comparison";

/// The C recursion checks a call of the `repr` builtin makes before the repr itself: one.
pub(crate) const REPR_CALL: usize = 1;

/// The C recursion checks `", ".join(repr(each) for each in values)` makes before each repr: the `str.join` call
/// (1), resuming the generator expression from C, which enters a new evaluation loop (2), and the `repr` call (1).
pub(crate) const JOINED_REPR_CALLS: usize = 1 + 2 + REPR_CALL;

/// The `tracing` target of the `warnings.warn` calls the Python SDK makes while validating.
pub(crate) const PYTHON_WARNINGS: &str = "trogon_claude_agent_sdk::python_warnings";

/// The message of the `DeprecationWarning` `_warn_for_remote_retrieve` emits.
pub(crate) const REMOTE_RETRIEVE_WARNING: &str = "Automatically retrieving remote references can be a security \
    vulnerability and is discouraged by the JSON Schema specifications. Relying on this behavior is deprecated and \
    will shortly become an error. If you are sure you want to remotely retrieve your reference and that it is safe \
    to do so, you can find instructions for doing so via referencing.Registry in the referencing documentation \
    (https://referencing.readthedocs.org).";

/// How deep `json.load` nests before the `RecursionError` CPython raises at its default recursion limit, with the
/// frames a `$ref` lookup in `jsonschema` already holds.
pub(crate) const JSON_DEPTH: usize = 9990;

/// `MAX_HASH_SEED` of `Python/initconfig.c`.
pub(crate) const MAX_HASH_SEED: u64 = 4_294_967_295;

/// `ftplib.MAXLINE`.
pub(crate) const MAX_LINE: usize = 8192;

/// The size of the chunks `TextIOWrapper` asks the socket for.
pub(crate) const CHUNK: usize = 8192;

/// `_MAXLINE`.
pub(crate) const HTTP_CLIENT_MAX_LINE: usize = 65536;

/// `_MAXHEADERS`.
pub(crate) const MAX_HEADERS: usize = 100;

/// `_MAXINTERIMRESPONSES`.
pub(crate) const MAX_INTERIM_RESPONSES: usize = 100;

/// The `User-Agent` `jsonschema` sends.
pub(crate) const USER_AGENT: &str = "python-jsonschema (deprecated $ref resolution)";

/// The pattern `PrettyPrinter._pprint_str` splits a line too long to fit with: `re.findall(r'\S*\s*', line)`.
pub(crate) const WORDS: &str = r"\S*\s*";

/// The frames below `re.findall` that compiling [`WORDS`] reaches while it is not in the `re` cache: `_compile`,
/// `_compiler.compile`, `_parser.parse`, `_parse_sub`, `_parse`, and the `Tokenizer.get` and `__next` it calls.
pub(crate) const WORDS_COMPILE_FRAMES: usize = 7;

/// `re._MAXCACHE`.
pub(crate) const MAX_CACHE: usize = 512;

/// `re._MAXCACHE2`.
pub(crate) const MAX_CACHE2: usize = 256;

/// The keys of `_cache` when the Python SDK first calls `jsonschema.validate`, oldest first, in a process whose script
/// imports `claude_agent_sdk` and connects on the pinned CPython: the patterns that the imports of
/// `claude_agent_sdk` and its dependencies compile, then the version pattern `subprocess_cli.py` matches while
/// connecting (lines 1148 and 1167). `_cache2` holds the same keys in the same order. A key of a `str` pattern
/// compiled with no flags is that pattern; `None` stands for one `jsonschema` never looks up: a `bytes` pattern, one
/// compiled with flags, or one with a lone surrogate.
pub(crate) const PROCESS: [Option<&str>; 150] = [
    None,
    None,
    None,
    Some("([\\\u{9}\\\u{a}\\\u{b}\\\u{c}\\\u{d}\\ ]+)"),
    Some("[a-z][\\.\\!\\?][\\\"\\']?\\Z"),
    None,
    None,
    None,
    None,
    None,
    Some("^(\\d+|\\w+)(\\.\\w+|\\[[^]]+\\])*$"),
    Some("%(?:\\((?P<key>.*?)\\))?(?P<modifiers>[-#0-9 +*.hlL]*?)[eEfFgGdiouxXcrs%]"),
    Some("(.*?(?:\\r\\n|\\n|\\r|$))"),
    Some("^(?:\\s*(\\w+)\\s*\\.)?\\s*(\\w+)"),
    None,
    Some("([\u{0}-\u{7f}]+)"),
    Some("^[a-zA-Z][a-zA-Z0-9_]*$"),
    None,
    Some("[.\u{3002}\u{ff0e}\u{ff61}]"),
    Some("[\u{0}-,/:-@A-Z[-`{-\u{7f}]"),
    None,
    None,
    None,
    None,
    Some("[\\x00-\\x1f\\\\\"\\b\\f\\n\\r\\t]"),
    Some("([\\\\\"]|[^\\ -~])"),
    None,
    None,
    None,
    Some("(?m)^    "),
    Some("((\\w+\\.)?Annotated\\[)?(\\w+\\.)?ClassVar\\["),
    Some("<key>ProductUserVisibleVersion</key>\\s*<string>(.*?)</string>"),
    Some("-isysroot\\s*(\\S+)"),
    None,
    Some("[][\\\\()<>@,:;\".]"),
    Some("[\\\\\"]"),
    None,
    None,
    Some("[\\041-\\176]+:$"),
    Some("\\n[^ \\t]+:"),
    None,
    Some("[ \\(\\)<>@,;:\\\\\"/\\[\\]\\?=]"),
    Some("(?m)^        "),
    Some("(?P<module>[\\w.]+)\\s*(:\\s*(?P<attr>[\\w.]+)\\s*)?((?P<extras>\\[.*\\])\\s*)?$"),
    Some(
        "\\s*(?:((?:[\\w.!#$%&\\'*+\\-/=?^_`{|}~]+\\s+)*[\\w.!#$%&\\'*+\\-/=?^_`{|}~]+)|\"((?:[^\"]|\\\")+)\")?\\s*<(.+)>\\s*",
    ),
    Some("[-_.]+"),
    Some("^[a-z]+[A-Za-z0-9]*$"),
    Some("([0-9A-Za-z])_(?=[0-9A-Z])"),
    Some("(^_*[A-Z])"),
    Some("\\d[a-z]"),
    None,
    None,
    Some("\\r\\n|\\r|\\n"),
    Some("(\\r\\n|\\r|\\n)"),
    Some("(\\r\\n|\\r|\\n)\\Z"),
    Some("^(From |[\\041-\\071\\073-\\176]*:|[\\t ])"),
    None,
    None,
    Some("[\u{0}- \u{7f}]"),
    Some("[\u{0}-\u{1f}]"),
    Some("[abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!\\#\\$%\\&'\\*\\+\\-\\.\\^_`\\|\\~:]+"),
    Some("[\\x00-\\x1F\\x7F]"),
    Some("\\\\(?:([0-3][0-7][0-7])|(.))"),
    None,
    Some("&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\\t\\n\\f <&#;]{1,32};?)"),
    Some("{([a-zA-Z_][a-zA-Z0-9_]*)(:[a-zA-Z_][a-zA-Z0-9_]*)?}"),
    Some("{([a-zA-Z_][a-zA-Z0-9_]*)}"),
    Some("[a-z][_0-9a-z\\-\\*\\/]{0,255}|[a-z0-9][_0-9a-z\\-\\*\\/]{0,240}@[a-z][_0-9a-z\\-\\*\\/]{0,13}"),
    Some("[\\x20-\\x2b\\x2d-\\x3c\\x3e-\\x7e]{0,255}[\\x21-\\x2b\\x2d-\\x3c\\x3e-\\x7e]"),
    Some(
        "([a-z][_0-9a-z\\-\\*\\/]{0,255}|[a-z0-9][_0-9a-z\\-\\*\\/]{0,240}@[a-z][_0-9a-z\\-\\*\\/]{0,13})=([\\x20-\\x2b\\x2d-\\x3c\\x3e-\\x7e]{0,255}[\\x21-\\x2b\\x2d-\\x3c\\x3e-\\x7e])",
    ),
    Some(
        "[ \\t]*[\\x21\\x23-\\x27\\x2a\\x2b\\x2d\\x2e\\x30-\\x39\\x41-\\x5a\\x5e-\\x7a\\x7c\\x7e]+[ \\t]*=[ \\t]*[\\x21\\x23-\\x2b\\x2d-\\x3a\\x3c-\\x5b\\x5d-\\x7e]*[ \\t]*",
    ),
    Some(
        "[ \\t]*[\\x21\\x23-\\x27\\x2a\\x2b\\x2d\\x2e\\x30-\\x39\\x41-\\x5a\\x5e-\\x7a\\x7c\\x7e]+[ \\t]*=[ \\t]*[\\x20\\x21\\x23-\\x2b\\x2d-\\x3a\\x3c-\\x5b\\x5d-\\x7e]*[ \\t]*",
    ),
    Some("[ \\t]*,[ \\t]*"),
    Some("[\\x21\\x23-\\x27\\x2a\\x2b\\x2d\\x2e\\x30-\\x39\\x41-\\x5a\\x5e-\\x7a\\x7c\\x7e]+"),
    Some("[\\x21\\x23-\\x2b\\x2d-\\x3a\\x3c-\\x5b\\x5d-\\x7e]*"),
    Some(
        "[ \\t]*[\\x21\\x23-\\x27\\x2a\\x2b\\x2d\\x2e\\x30-\\x39\\x41-\\x5a\\x5e-\\x7a\\x7c\\x7e]+[ \\t]*=[ \\t]*[\\x21\\x23-\\x2b\\x2d-\\x3a\\x3c-\\x5b\\x5d-\\x7e]*[ \\t]*|[ \\t]*[\\x21\\x23-\\x27\\x2a\\x2b\\x2d\\x2e\\x30-\\x39\\x41-\\x5a\\x5e-\\x7a\\x7c\\x7e]+[ \\t]*",
    ),
    Some("^[ \u{9}]*([0-9a-f]{2})-([0-9a-f]{32})-([0-9a-f]{16})-([0-9a-f]{2})(?:(-.*)|[ \u{9}]*)$"),
    Some("[\\w\\.]+"),
    Some("^=\\?base64\\?(?P<payload>.*)\\?=$"),
    Some("^[\\x20-\\x7E]*$"),
    Some("^[!#$%&'*+\\-.^_`|~0-9A-Za-z]+$"),
    Some("^-?[0-9]+(\\.[0-9]+)?$"),
    Some("[A-Z][A-Z0-9_]+$"),
    None,
    Some("^(?P<prefix>[a-z]+)://(?P<suffix>.*)$"),
    Some("^\\s*(\\w+)\\s*"),
    Some("^\\.\\s*(\\w+)\\s*"),
    Some("^\\[\\s*(\\w+)\\s*\\]\\s*"),
    Some("^\\d+$"),
    Some("%\\(([^)]+)\\)s"),
    Some("\\$\\{([^}]+)\\}"),
    Some("%\\(([^)]*)\\)s|."),
    None,
    None,
    None,
    Some("\\S"),
    Some("^get(?P<name>.+)$"),
    None,
    Some("\\033\\[[0-?]*[ -/]*[@-~]"),
    Some("^[\\x21-\\x7E]+$"),
    Some(
        "[A-Za-z](?:[A-Za-z0-9-]*[A-Za-z0-9])?(?:\\.[A-Za-z](?:[A-Za-z0-9-]*[A-Za-z0-9])?)*/[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?",
    ),
    None,
    None,
    Some("#( Netscape)? HTTP Cookie File"),
    None,
    None,
    None,
    None,
    None,
    Some("^\\s*([^=\\s;,]+)"),
    Some("^\\s*=\\s*\\\"([^\\\"\\\\]*(?:\\\\.[^\\\"\\\\]*)*)\\\""),
    Some("^\\s*=\\s*([^\\s;,]*)"),
    Some("\\\\(.)"),
    Some("([\\\"\\\\])"),
    None,
    Some("%([0-9a-fA-F][0-9a-fA-F])"),
    Some("\\W"),
    Some("\\.?[^.]*"),
    Some("[^.]*"),
    Some("^\\.+"),
    None,
    Some(
        "\"|\\\\|\u{0}|\u{1}|\u{2}|\u{3}|\u{4}|\u{5}|\u{6}|\u{7}|\u{8}|\\\u{9}|\\\u{a}|\\\u{b}|\\\u{c}|\\\u{d}|\u{e}|\u{f}|\u{10}|\u{11}|\u{12}|\u{13}|\u{14}|\u{15}|\u{16}|\u{17}|\u{18}|\u{19}|\u{1a}|\u{1c}|\u{1d}|\u{1e}|\u{1f}",
    ),
    Some("[!#$%&'*+\\-.^_`|~0-9A-Za-z]+"),
    Some("[\\x00-\\x08\\x0a-\\x1f\\x7f]"),
    Some("%[A-Fa-f0-9]{2}"),
    Some(
        "(?:(?P<scheme>([a-zA-Z][a-zA-Z0-9+.-]*)?):)?(?://(?P<authority>[^/?#]*))?(?P<path>[^?#]*)(?:\\?(?P<query>[^#]*))?(?:#(?P<fragment>.*))?",
    ),
    Some("(?:(?P<userinfo>.*)@)?(?P<host>(\\[.*\\]|[^:@]*)):?(?P<port>.*)?"),
    Some("([a-zA-Z][a-zA-Z0-9+.-]*)?"),
    Some("[^/?#]*"),
    Some("[^?#]*"),
    Some("[^#]*"),
    Some(".*"),
    Some("[^@]*"),
    Some("(\\[.*\\]|[^:]*)"),
    Some("^[0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+$"),
    Some("^\\[.*\\]$"),
    Some("^[A-Za-z0-9_]+(?:\\.[A-Za-z0-9_]+)*$"),
    Some("%[0-9A-Fa-f]{2}"),
    Some("^[A-Za-z0-9._-]{1,128}$"),
    None,
    Some(
        "^(?:<local-command-stdout>|<session-start-hook>|<tick>|<goal>|\\[Request interrupted by user[^\\]]*\\]|\\s*<ide_opened_file>[\\s\\S]*</ide_opened_file>\\s*$|\\s*<ide_selection>[\\s\\S]*</ide_selection>\\s*$)",
    ),
    Some("<command-name>(.*?)</command-name>"),
    Some("[^a-zA-Z0-9]"),
    Some("[\u{200b}-\u{200f}\u{202a}-\u{202e}\u{2066}-\u{2069}\u{feff}\u{e000}-\u{f8ff}]"),
    Some("(?:@[A-Za-z0-9._-]+/)?[A-Za-z0-9][A-Za-z0-9._-]*(?:/[A-Za-z0-9][A-Za-z0-9.!+_-]*)?"),
    None,
    Some(";.*\\bextra\\s*=="),
    Some("[(),\\x00-\\x1f\\x7f-\\x9f\\ufeff]"),
    None,
    Some("(\\d{1,9})\\.(\\d{1,9})\\.(\\d{1,9})"),
];

/// The alphabet of `base64.b64encode`.
pub(crate) const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `HTTPRedirectHandler.max_repeats`.
pub(crate) const MAX_REPEATS: u32 = 4;

/// `HTTPRedirectHandler.max_redirections`.
pub(crate) const MAX_REDIRECTIONS: usize = 10;

/// `HTTPRedirectHandler.inf_msg`.
pub(crate) const INFINITE_LOOP: &str = "The HTTP server returned a redirect error that would lead to an infinite loop.\n\
                             The last 30x error message was:\n";

/// The `OverflowError` `chr` raises for a code point past a C `int`, which a `\U` escape can name.
pub(crate) const CHR_TOO_LARGE: &str = "Python int too large to convert to C int";

/// The `OverflowError` a repetition bound at or past `MAXREPEAT` raises.
pub(crate) const REPEAT_TOO_LARGE: &str = "the repetition number is too large";

/// The `ValueError` of `fix_flags` for a str pattern with both the ASCII and UNICODE flags.
pub(crate) const FLAGS_INCOMPATIBLE: &str = "ASCII and UNICODE flags are incompatible";

/// How the `ValueError` of the `int()` digit limit starts.
pub(crate) const DIGIT_LIMIT_PREFIX: &str = "Exceeds the limit (";

/// What Python reports when the server closes the connection in the middle of the handshake.
pub(crate) const EOF: &str = "[SSL: UNEXPECTED_EOF_WHILE_READING] EOF occurred in violation of protocol";

/// What OpenSSL reports for a record it cannot take, including one that does not decrypt.
pub(crate) const RECORD_LAYER_FAILURE: &str = "[SSL] record layer failure";

/// `SSLZeroReturnError`, a `close_notify` alert.
pub(crate) const CLOSED: &str = "TLS/SSL connection has been closed (EOF)";

/// Used by the python_urlopen_tls_gate port.
pub(crate) const INTERNAL_ERROR: &str = "[SSL] internal error";

/// Used by the python_urlopen_tls_gate port.
pub(crate) const MISSING_SUPPORTED_VERSIONS: &str = "[SSL] missing supported versions extension";

/// The alert descriptions OpenSSL names, with the text a fatal one raises.
pub(crate) const ALERTS: &[(u8, &str)] = &[
    (
        10,
        "[SSL: SSLV3_ALERT_UNEXPECTED_MESSAGE] ssl/tls alert unexpected message",
    ),
    (20, "[SSL: SSLV3_ALERT_BAD_RECORD_MAC] ssl/tls alert bad record mac"),
    (21, "[SSL: TLSV1_ALERT_DECRYPTION_FAILED] tlsv1 alert decryption failed"),
    (22, "[SSL: TLSV1_ALERT_RECORD_OVERFLOW] tlsv1 alert record overflow"),
    (
        30,
        "[SSL: SSLV3_ALERT_DECOMPRESSION_FAILURE] ssl/tls alert decompression failure",
    ),
    (
        40,
        "[SSL: SSLV3_ALERT_HANDSHAKE_FAILURE] ssl/tls alert handshake failure",
    ),
    (41, "[SSL: SSLV3_ALERT_NO_CERTIFICATE] ssl/tls alert no certificate"),
    (42, "[SSL: SSLV3_ALERT_BAD_CERTIFICATE] ssl/tls alert bad certificate"),
    (
        43,
        "[SSL: SSLV3_ALERT_UNSUPPORTED_CERTIFICATE] ssl/tls alert unsupported certificate",
    ),
    (
        44,
        "[SSL: SSLV3_ALERT_CERTIFICATE_REVOKED] ssl/tls alert certificate revoked",
    ),
    (
        45,
        "[SSL: SSLV3_ALERT_CERTIFICATE_EXPIRED] ssl/tls alert certificate expired",
    ),
    (
        46,
        "[SSL: SSLV3_ALERT_CERTIFICATE_UNKNOWN] ssl/tls alert certificate unknown",
    ),
    (
        47,
        "[SSL: SSLV3_ALERT_ILLEGAL_PARAMETER] ssl/tls alert illegal parameter",
    ),
    (48, "[SSL: TLSV1_ALERT_UNKNOWN_CA] tlsv1 alert unknown ca"),
    (49, "[SSL: TLSV1_ALERT_ACCESS_DENIED] tlsv1 alert access denied"),
    (50, "[SSL: TLSV1_ALERT_DECODE_ERROR] tlsv1 alert decode error"),
    (51, "[SSL: TLSV1_ALERT_DECRYPT_ERROR] tlsv1 alert decrypt error"),
    (
        60,
        "[SSL: TLSV1_ALERT_EXPORT_RESTRICTION] tlsv1 alert export restriction",
    ),
    (70, "[SSL: TLSV1_ALERT_PROTOCOL_VERSION] tlsv1 alert protocol version"),
    (
        71,
        "[SSL: TLSV1_ALERT_INSUFFICIENT_SECURITY] tlsv1 alert insufficient security",
    ),
    (80, "[SSL: TLSV1_ALERT_INTERNAL_ERROR] tlsv1 alert internal error"),
    (
        86,
        "[SSL: TLSV1_ALERT_INAPPROPRIATE_FALLBACK] tlsv1 alert inappropriate fallback",
    ),
    (90, "[SSL: TLSV1_ALERT_USER_CANCELLED] tlsv1 alert user cancelled"),
    (100, "[SSL: TLSV1_ALERT_NO_RENEGOTIATION] tlsv1 alert no renegotiation"),
    (
        109,
        "[SSL: TLSV13_ALERT_MISSING_EXTENSION] tlsv13 alert missing extension",
    ),
    (110, "[SSL: TLSV1_UNSUPPORTED_EXTENSION] tlsv1 unsupported extension"),
    (
        111,
        "[SSL: TLSV1_CERTIFICATE_UNOBTAINABLE] tlsv1 certificate unobtainable",
    ),
    (112, "[SSL: TLSV1_UNRECOGNIZED_NAME] tlsv1 unrecognized name"),
    (
        113,
        "[SSL: TLSV1_BAD_CERTIFICATE_STATUS_RESPONSE] tlsv1 bad certificate status response",
    ),
    (
        114,
        "[SSL: TLSV1_BAD_CERTIFICATE_HASH_VALUE] tlsv1 bad certificate hash value",
    ),
    (115, "[SSL] tlsv1 alert unknown psk identity"),
    (
        116,
        "[SSL: TLSV13_ALERT_CERTIFICATE_REQUIRED] tlsv13 alert certificate required",
    ),
    (120, "[SSL] tlsv1 alert no application protocol"),
];

/// The random of a hello retry request.
pub(crate) const HELLO_RETRY_RANDOM: [u8; 32] = [
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91, 0xc2, 0xa2, 0x11,
    0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
];

/// The cipher suites Python's default context offers.
pub(crate) const SENT_SUITES: [u16; 17] = [
    0x1302, 0x1303, 0x1301, 0xc02c, 0xc030, 0xc02b, 0xc02f, 0xcca9, 0xcca8, 0xc024, 0xc028, 0xc023, 0xc027, 0x009f,
    0x009e, 0x006b, 0x0067,
];

/// The groups Python's default context supports.
pub(crate) const SUPPORTED_GROUPS: [u16; 8] = [0x11ec, 0x001d, 0x0017, 0x001e, 0x0018, 0x0019, 0x0100, 0x0101];

/// The groups `rustls` cannot use. A retry request for one of them leaves `rustls` on a group it can use, while the
/// gate keeps checking the server's answers against the group it asked for.
pub(crate) const DETACHED_GROUPS: [u16; 4] = [0x0019, 0x001e, 0x0100, 0x0101];

/// The lengths OpenSSL checks key shares of the groups `rustls` cannot use against.
pub(crate) const SHARE_LENGTHS: [(u16, usize); 4] = [(0x001e, 56), (0x0019, 133), (0x0100, 256), (0x0101, 384)];

/// The cipher suites OpenSSL 3.5 knows, as inclusive ranges.
pub(crate) const KNOWN_RANGES: &[(u16, u16)] = &[
    (0x0001, 0x0002),
    (0x0007, 0x0007),
    (0x002c, 0x002f),
    (0x0032, 0x0035),
    (0x0038, 0x003d),
    (0x0040, 0x0041),
    (0x0044, 0x0046),
    (0x0067, 0x0067),
    (0x006a, 0x006d),
    (0x0081, 0x0081),
    (0x0083, 0x0084),
    (0x0087, 0x0089),
    (0x008c, 0x008d),
    (0x0090, 0x0091),
    (0x0094, 0x0096),
    (0x0099, 0x009f),
    (0x00a2, 0x00a3),
    (0x00a6, 0x00ba),
    (0x00bd, 0x00c0),
    (0x00c3, 0x00c5),
    (0x1301, 0x1305),
    (0xc006, 0xc006),
    (0xc009, 0xc00a),
    (0xc010, 0xc010),
    (0xc013, 0xc015),
    (0xc018, 0xc019),
    (0xc01d, 0xc024),
    (0xc027, 0xc028),
    (0xc02b, 0xc02c),
    (0xc02f, 0xc030),
    (0xc035, 0xc03b),
    (0xc050, 0xc053),
    (0xc056, 0xc057),
    (0xc05c, 0xc05d),
    (0xc060, 0xc061),
    (0xc06a, 0xc06f),
    (0xc072, 0xc073),
    (0xc076, 0xc077),
    (0xc094, 0xc0af),
    (0xc0b4, 0xc0b5),
    (0xc100, 0xc102),
    (0xcca8, 0xccae),
    (0xff85, 0xff85),
    (0xff87, 0xff87),
];

/// The messages an extension may appear in, as OpenSSL's `SSL_EXT_*` context bits.
pub(crate) const CLIENT_HELLO: u16 = 1;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const SERVER_HELLO_12: u16 = 2;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const SERVER_HELLO_13: u16 = 4;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const RETRY_REQUEST: u16 = 8;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const ENCRYPTED_EXTENSIONS: u16 = 16;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const CERTIFICATE_13: u16 = 32;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const CERTIFICATE_REQUEST_13: u16 = 64;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const NEW_SESSION_TICKET_13: u16 = 128;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const TLS12_ONLY: u16 = 256;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const TLS13_ONLY: u16 = 512;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const DTLS_ONLY: u16 = 1024;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const IMPLEMENTATION_ONLY: u16 = 2048;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const SSL3_ALLOWED: u16 = 4096;

/// OpenSSL's table of built-in extensions, in the order it parses them, with their contexts.
pub(crate) const KNOWN_EXTENSIONS: &[(u16, u16)] = &[
    (0xff01, CLIENT_HELLO | SERVER_HELLO_12 | TLS12_ONLY | SSL3_ALLOWED),
    (0x0000, CLIENT_HELLO | SERVER_HELLO_12 | ENCRYPTED_EXTENSIONS),
    (0x0001, CLIENT_HELLO | SERVER_HELLO_12 | ENCRYPTED_EXTENSIONS),
    (0x000c, CLIENT_HELLO | TLS12_ONLY),
    (0x000b, CLIENT_HELLO | SERVER_HELLO_12 | TLS12_ONLY),
    (0x000a, CLIENT_HELLO | SERVER_HELLO_12 | ENCRYPTED_EXTENSIONS),
    (0x0023, CLIENT_HELLO | SERVER_HELLO_12 | TLS12_ONLY),
    (
        0x0005,
        CLIENT_HELLO | SERVER_HELLO_12 | CERTIFICATE_13 | CERTIFICATE_REQUEST_13,
    ),
    (0x3374, CLIENT_HELLO | SERVER_HELLO_12 | TLS12_ONLY),
    (0x0010, CLIENT_HELLO | SERVER_HELLO_12 | ENCRYPTED_EXTENSIONS),
    (
        0x000e,
        CLIENT_HELLO | SERVER_HELLO_12 | ENCRYPTED_EXTENSIONS | DTLS_ONLY,
    ),
    (0x0016, CLIENT_HELLO | SERVER_HELLO_12 | TLS12_ONLY),
    (
        0x0012,
        CLIENT_HELLO | SERVER_HELLO_12 | CERTIFICATE_13 | CERTIFICATE_REQUEST_13,
    ),
    (0x0017, CLIENT_HELLO | SERVER_HELLO_12 | TLS12_ONLY),
    (0x0032, CLIENT_HELLO | CERTIFICATE_REQUEST_13),
    (0x0031, CLIENT_HELLO | TLS13_ONLY),
    (0x0013, CLIENT_HELLO | SERVER_HELLO_12 | ENCRYPTED_EXTENSIONS),
    (0x0014, CLIENT_HELLO | SERVER_HELLO_12 | ENCRYPTED_EXTENSIONS),
    (0x000d, CLIENT_HELLO | CERTIFICATE_REQUEST_13),
    (
        0x002b,
        CLIENT_HELLO | SERVER_HELLO_13 | RETRY_REQUEST | IMPLEMENTATION_ONLY,
    ),
    (0x002d, CLIENT_HELLO | TLS13_ONLY | IMPLEMENTATION_ONLY),
    (
        0x0033,
        CLIENT_HELLO | SERVER_HELLO_13 | RETRY_REQUEST | TLS13_ONLY | IMPLEMENTATION_ONLY,
    ),
    (0x002c, CLIENT_HELLO | RETRY_REQUEST | TLS13_ONLY | IMPLEMENTATION_ONLY),
    (0xfde8, CLIENT_HELLO | SERVER_HELLO_12 | TLS12_ONLY),
    (
        0x001b,
        CLIENT_HELLO | CERTIFICATE_REQUEST_13 | TLS13_ONLY | IMPLEMENTATION_ONLY,
    ),
    (
        0x002a,
        CLIENT_HELLO | ENCRYPTED_EXTENSIONS | NEW_SESSION_TICKET_13 | TLS13_ONLY,
    ),
    (0x002f, CLIENT_HELLO | CERTIFICATE_REQUEST_13 | TLS13_ONLY),
    (0x0015, CLIENT_HELLO),
    (
        0x0029,
        CLIENT_HELLO | SERVER_HELLO_13 | TLS13_ONLY | IMPLEMENTATION_ONLY,
    ),
];

/// The extensions a server may answer. Python's client hello does not ask for `status_request`, but `rustls` always
/// does, so a genuine server may acknowledge it.
pub(crate) const SENT_EXTENSIONS: [u16; 13] = [5, 0xff01, 11, 10, 35, 16, 22, 23, 49, 13, 43, 45, 51];

/// The HTTP methods OpenSSL recognises at the start of a server's first bytes.
pub(crate) const HTTP_METHODS: [&[u8]; 8] = [
    b"GET ", b"POST ", b"HEAD ", b"PATCH", b"OPTIO", b"DELET", b"TRACE", b"PUT ",
];

/// The handshake type the gate gives a change cipher spec, outside the range of real ones.
pub(crate) const CHANGE_CIPHER_SPEC: u16 = 0x100;

/// The largest record body OpenSSL takes before encryption, and the largest it takes encrypted.
pub(crate) const MAX_PLAINTEXT: usize = 16384;

/// Used by the python_urlopen_tls_gate port.
pub(crate) const MAX_CIPHERTEXT: usize = 16640;

/// Where the session id of the client hello sits in the bytes `rustls` writes: after the record header, the handshake
/// header, the version and the random.
pub(crate) const SESSION_ID_AT: usize = 43;

/// `PySet_MINSIZE`.
pub(crate) const MIN_SIZE: usize = 8;

/// `LINEAR_PROBES`.
pub(crate) const LINEAR_PROBES: usize = 9;

/// `PERTURB_SHIFT`.
pub(crate) const PERTURB_SHIFT: u32 = 5;

/// The `str` of the `ValueError` `str()` raises for an `int` of more than `sys.get_int_max_str_digits()` digits.
pub(crate) const INT_STR_LIMIT_TEXT: &str = "Exceeds the limit (4300 digits) for integer string conversion; use \
                                             sys.set_int_max_str_digits() to increase the limit";

/// The text of the `OverflowError` a size below `ssize_t` raises.
pub(crate) const READ_SIZE_OVERFLOW: &str = "cannot fit 'int' into an index-sized integer";

/// The OpenSSL source location CPython 3.12 reports for handshake failures.
pub(crate) const SSL_LOCATION: &str = "(_ssl.c:1011)";

/// Nesting under which the recursion limit of `serde_json` (127 levels) never trips, so its recursive walks run in
/// place.
pub(crate) const SHALLOW: usize = 64;

/// The stack of the thread that finds the error of a malformed message nested deeper than [`SHALLOW`].
pub(crate) const PARSE_STACK: usize = 512 << 20;

/// Containers the Python SDK's message reader enters with `json.loads` before CPython 3.12's C recursion limit
/// raises: the C scanner checks the limit once per object or array, and the reader task already holds the rest.
pub(crate) const PYTHON_READER_DEPTH: usize = 9991;

/// The most anyio's `TextReceiveStream` takes from the pipe in one `receive`.
pub(crate) const PYTHON_CHUNK: usize = 65536;

/// `mimetypes.knownfiles`. On Windows only the registry is read, which raises only when `HKEY_CLASSES_ROOT` cannot
/// be opened for reading.
#[cfg(not(windows))]
pub(crate) const MIME_FILES: [&str; 9] = [
    "/etc/mime.types",
    "/etc/httpd/mime.types",
    "/etc/httpd/conf/mime.types",
    "/etc/apache/mime.types",
    "/etc/apache2/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/lib/netscape/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/etc/mime.types",
];
#[cfg(windows)]
pub(crate) const MIME_FILES: [&str; 0] = [];

/// What `TextIOWrapper` asks `BufferedReader.read1` for, which an empty buffer reads from the file in one call.
pub(crate) const TEXT_CHUNK: usize = 8192;

/// The code points CPython 3.12's `str.lower()` (Unicode 15.0) leaves as they are and `char::to_lowercase` (Unicode
/// 17.0 in Rust 1.96) maps: the case pairs Unicode 16.0 added. Everywhere else the two answer the same, which the
/// golden over every code point in `python_case.json` checks.
pub(crate) const LOWER_SINCE_15: [RangeInclusive<char>; 9] = [
    '\u{1c89}'..='\u{1c89}',
    '\u{a7cb}'..='\u{a7cc}',
    '\u{a7ce}'..='\u{a7ce}',
    '\u{a7d2}'..='\u{a7d2}',
    '\u{a7d4}'..='\u{a7d4}',
    '\u{a7da}'..='\u{a7da}',
    '\u{a7dc}'..='\u{a7dc}',
    '\u{10d50}'..='\u{10d65}',
    '\u{16ea0}'..='\u{16eb8}',
];

/// The code points CPython 3.12's `str.upper()` leaves as they are and `char::to_uppercase` maps.
pub(crate) const UPPER_SINCE_15: [RangeInclusive<char>; 10] = [
    '\u{19b}'..='\u{19b}',
    '\u{264}'..='\u{264}',
    '\u{1c8a}'..='\u{1c8a}',
    '\u{a7cd}'..='\u{a7cd}',
    '\u{a7cf}'..='\u{a7cf}',
    '\u{a7d3}'..='\u{a7d3}',
    '\u{a7d5}'..='\u{a7d5}',
    '\u{a7db}'..='\u{a7db}',
    '\u{10d70}'..='\u{10d85}',
    '\u{16ebb}'..='\u{16ed3}',
];

/// The code points that are `Cased` or `Case_Ignorable` in Rust's tables and neither in Unicode 15.0.
pub(crate) const CONTEXT_SINCE_15: [RangeInclusive<char>; 38] = [
    '\u{897}'..='\u{897}',
    '\u{1acf}'..='\u{1add}',
    '\u{1ae0}'..='\u{1aeb}',
    '\u{1c89}'..='\u{1c8a}',
    '\u{a7cb}'..='\u{a7cf}',
    '\u{a7d2}'..='\u{a7d2}',
    '\u{a7d4}'..='\u{a7d4}',
    '\u{a7da}'..='\u{a7dc}',
    '\u{a7f1}'..='\u{a7f1}',
    '\u{10d4e}'..='\u{10d4e}',
    '\u{10d50}'..='\u{10d65}',
    '\u{10d69}'..='\u{10d6d}',
    '\u{10d6f}'..='\u{10d85}',
    '\u{10ec5}'..='\u{10ec5}',
    '\u{10efa}'..='\u{10efc}',
    '\u{113bb}'..='\u{113c0}',
    '\u{113ce}'..='\u{113ce}',
    '\u{113d0}'..='\u{113d0}',
    '\u{113d2}'..='\u{113d2}',
    '\u{113e1}'..='\u{113e2}',
    '\u{11b60}'..='\u{11b60}',
    '\u{11b62}'..='\u{11b64}',
    '\u{11b66}'..='\u{11b66}',
    '\u{11dd9}'..='\u{11dd9}',
    '\u{11f5a}'..='\u{11f5a}',
    '\u{1611e}'..='\u{16129}',
    '\u{1612d}'..='\u{1612f}',
    '\u{16d40}'..='\u{16d42}',
    '\u{16d6b}'..='\u{16d6c}',
    '\u{16ea0}'..='\u{16eb8}',
    '\u{16ebb}'..='\u{16ed3}',
    '\u{16ff2}'..='\u{16ff3}',
    '\u{1e5ee}'..='\u{1e5ef}',
    '\u{1e6e3}'..='\u{1e6e3}',
    '\u{1e6e6}'..='\u{1e6e6}',
    '\u{1e6ee}'..='\u{1e6ef}',
    '\u{1e6f5}'..='\u{1e6f5}',
    '\u{1e6ff}'..='\u{1e6ff}',
];

/// The recorded tables: sections of hexadecimal ranges and mappings.
pub(crate) const TABLES: &str = include_str!("upstream/python_nameprep.txt");

/// The `UnicodeError` text for a label of no or too many bytes.
pub(crate) const EMPTY_OR_TOO_LONG: &str = "label empty or too long";

/// `sace_prefix`, as code points.
pub(crate) const ACE_PREFIX: [u32; 4] = [0x78, 0x6E, 0x2D, 0x2D];

/// The label separators of the `dots` pattern: full stop, ideographic, fullwidth and halfwidth full stops.
pub(crate) const DOTS: [u32; 4] = [0x2E, 0x3002, 0xFF0E, 0xFF61];

/// The Hangul constants of `unicodedata.c`.
pub(crate) const S_BASE: u32 = 0xAC00;
pub(crate) const L_BASE: u32 = 0x1100;
pub(crate) const V_BASE: u32 = 0x1161;
pub(crate) const T_BASE: u32 = 0x11A7;
pub(crate) const L_COUNT: u32 = 19;
pub(crate) const V_COUNT: u32 = 21;
pub(crate) const T_COUNT: u32 = 28;
pub(crate) const N_COUNT: u32 = V_COUNT * T_COUNT;
pub(crate) const S_COUNT: u32 = L_COUNT * N_COUNT;

/// `punycode.digits`.
pub(crate) const PUNYCODE_DIGITS: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";

#[cfg(any(test, target_os = "macos", windows))]
pub(crate) const FNMATCH_STAR: u32 = '*' as u32;
#[cfg(any(test, target_os = "macos", windows))]
pub(crate) const FNMATCH_QUESTION: u32 = '?' as u32;
#[cfg(any(test, target_os = "macos", windows))]
pub(crate) const FNMATCH_OPEN: u32 = '[' as u32;
#[cfg(any(test, target_os = "macos", windows))]
pub(crate) const FNMATCH_CLOSE: u32 = ']' as u32;
#[cfg(any(test, target_os = "macos", windows))]
pub(crate) const FNMATCH_BANG: u32 = '!' as u32;
#[cfg(any(test, target_os = "macos", windows))]
pub(crate) const FNMATCH_CARET: u32 = '^' as u32;
#[cfg(any(test, target_os = "macos", windows))]
pub(crate) const FNMATCH_HYPHEN: u32 = '-' as u32;
#[cfg(any(test, target_os = "macos", windows))]
pub(crate) const FNMATCH_BACKSLASH: u32 = '\\' as u32;

/// The characters `re.escape` puts a backslash before.
#[cfg(any(test, target_os = "macos", windows))]
pub(crate) const RE_ESCAPE_SPECIAL: &[u8] = b"()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c";

/// The protocols `_get_proxies` reads, in its order.
#[cfg(any(test, target_os = "macos"))]
pub(crate) const PROXY_PROTOCOLS: [&str; 5] = ["http", "https", "ftp", "gopher", "socks"];

/// `kCFNumberSInt32Type`.
#[cfg(target_os = "macos")]
pub(crate) const CF_NUMBER_SINT32_TYPE: isize = 3;

/// `HKEY_CURRENT_USER`: `(HKEY)(ULONG_PTR)((LONG)0x80000001)`, the `LONG` sign-extended to the width of a pointer.
#[cfg(windows)]
pub(crate) const HKEY_CURRENT_USER: isize = -0x7FFF_FFFF;
/// `KEY_READ`, the access `winreg.OpenKey` asks for by default.
#[cfg(windows)]
pub(crate) const KEY_READ: u32 = 0x2_0019;
#[cfg(windows)]
pub(crate) const ERROR_SUCCESS: i32 = 0;
#[cfg(windows)]
pub(crate) const ERROR_MORE_DATA: i32 = 234;
/// `ComputerNamePhysicalDnsHostname`, the name `socket.gethostname()` asks `GetComputerNameExW` for.
#[cfg(windows)]
pub(crate) const COMPUTER_NAME_PHYSICAL_DNS_HOSTNAME: i32 = 5;
/// `MAX_COMPUTERNAME_LENGTH`: CPython's first `GetComputerNameExW` buffer holds one unit more.
#[cfg(windows)]
pub(crate) const MAX_COMPUTERNAME_LENGTH: u32 = 15;
/// The buffer `socket_gethostname` (CPython 3.12.16 `Modules/socketmodule.c`) hands `gethostname`, which may fill all
/// of it but the last byte.
#[cfg(unix)]
pub(crate) const GETHOSTNAME_BUFFER: usize = 1024;
/// `LCMAP_LOWERCASE`.
#[cfg(windows)]
pub(crate) const LCMAP_LOWERCASE: u32 = 0x100;
/// `FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS`.
#[cfg(windows)]
pub(crate) const FORMAT_MESSAGE_FLAGS: u32 = 0x100 | 0x1000 | 0x200;
/// `MAKELANGID(LANG_NEUTRAL, SUBLANG_DEFAULT)`.
#[cfg(windows)]
pub(crate) const DEFAULT_LANGUAGE: u32 = 0x400;

#[cfg(windows)]
pub(crate) const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// `winreg` value types, as `Reg2Py` of `PC/winreg.c` converts them.
#[cfg(any(test, windows))]
pub(crate) const REG_SZ: u32 = 1;
#[cfg(any(test, windows))]
pub(crate) const REG_EXPAND_SZ: u32 = 2;
#[cfg(any(test, windows))]
pub(crate) const REG_DWORD: u32 = 4;
#[cfg(any(test, windows))]
pub(crate) const REG_MULTI_SZ: u32 = 7;
#[cfg(any(test, windows))]
pub(crate) const REG_QWORD: u32 = 11;

/// The bytes a file starts with when the TypeScript settings reader decodes it as UTF-16LE.
pub(crate) const UTF16LE_BOM: &[u8] = b"\xff\xfe";

/// The UTF-8 byte order mark the `utf-8-sig` codec drops from the start of a Python settings file.
pub(crate) const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";
