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

/// The most decimal digits Python's `int()` converts from a string (`sys.get_int_max_str_digits()`).
pub(crate) const PYTHON_INT_MAX_STR_DIGITS: usize = 4300;

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

/// The sandbox mount that always counts as a connector download folder (`z8e` upstream).
pub(crate) const CONNECTOR_DOWNLOADS_ROOT: &str = "/mnt/user-data/uploads/connector-downloads";

/// Whole contents of a `.gitignore` that marks a connector download folder as ignored (`n7e` upstream).
pub(crate) const CONNECTOR_GITIGNORE_MARKERS: [&[u8]; 3] = [b"*", b"*\n", b"*\r\n"];

/// Largest `.gitignore` the connector check reads (`t7e` upstream).
pub(crate) const CONNECTOR_GITIGNORE_MAX_BYTES: u64 = 3;

/// Entries the connector folder check remembers before it forgets all of them (`nE` upstream).
pub(crate) const CONNECTOR_CACHE_LIMIT: usize = 64;

/// Names of the temporary files a connector download leaves behind (`e7e` upstream).
pub(crate) const CONNECTOR_DOWNLOAD_TEMP_PATTERN: &str = r"^(?:download-[0-9a-f]{8}(?:-[0-9]+)?(?:\.[^./\\]*)?|\.download-[0-9a-f]{8}(?:-[0-9]+)?(?:\.[^./\\]*)?\.(?:from\.json|part))$";

/// Symbolic links the path walk follows before it gives up on the path.
pub(crate) const LINK_HOP_LIMIT: usize = 64;

/// `ENAMETOOLONG` as the host reports it.
#[cfg(target_os = "macos")]
pub(crate) const NAME_TOO_LONG_ERRNO: i32 = 63;

/// `ENAMETOOLONG` as the host reports it.
#[cfg(not(target_os = "macos"))]
pub(crate) const NAME_TOO_LONG_ERRNO: i32 = 36;

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

/// The longest tool name the TypeScript `McpServer` registers without printing a warning.
pub(crate) const MCP_TS_TOOL_NAME_MAX_LENGTH: usize = 128;
