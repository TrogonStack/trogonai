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

/// Settings keys whose arrays are replaced rather than unioned.
pub(crate) const REPLACED_ARRAY_KEYS: [&str; 1] = ["fallbackModel"];

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

/// Top-level key order of the upstream settings schema; keys outside it follow in input order.
pub(crate) const SETTINGS_SCHEMA_KEY_ORDER: [&str; 191] = [
    "$schema",
    "apiKeyHelper",
    "proxyAuthHelper",
    "awsCredentialExport",
    "awsAuthRefresh",
    "gcpAuthRefresh",
    "processWrapper",
    "policyHelper",
    "policyHelpers",
    "fileSuggestion",
    "respectGitignore",
    "breakReminder",
    "quietHours",
    "cleanupPeriodDays",
    "desktopSessionCleanupPeriodDays",
    "syncClaudeAiSkills",
    "syncClaudeAiPlugins",
    "skillListingMaxDescChars",
    "skillListingBudgetFraction",
    "wslInheritsWindowsSettings",
    "env",
    "attribution",
    "attributionProject",
    "attributionProjectPolicy",
    "includeCoAuthoredBy",
    "includeGitInstructions",
    "permissions",
    "model",
    "fallbackModel",
    "availableModels",
    "enforceAvailableModels",
    "availableModelsMatch",
    "deniedModels",
    "modelOverrides",
    "modelPicker",
    "modelPricing",
    "enableAllProjectMcpServers",
    "enabledMcpjsonServers",
    "disabledMcpjsonServers",
    "disableClaudeAiConnectors",
    "skillOverrides",
    "disableBundledSkills",
    "managedMcpServers",
    "allowedMcpServers",
    "deniedMcpServers",
    "hooks",
    "worktree",
    "disableAllHooks",
    "disableReplyCheck",
    "disableAgentView",
    "disableRemoteControl",
    "disableWorkflows",
    "disableArtifact",
    "enableArtifact",
    "enableWorkflows",
    "workflowSizeGuideline",
    "workflowKeywordTriggerEnabled",
    "disableSkillShellExecution",
    "disableWindowsBashCommandSplit",
    "defaultShell",
    "bashEditDiffEnabled",
    "bashOutputMaxChars",
    "taskOutputMaxChars",
    "respondToBashCommands",
    "allowManagedHooksOnly",
    "allowedHttpHookUrls",
    "httpHookAllowedEnvVars",
    "allowManagedPermissionRulesOnly",
    "allowManagedMcpServersOnly",
    "allowAllClaudeAiMcps",
    "allowClaudeInChromeWithManagedMcp",
    "strictPluginOnlyCustomization",
    "statusLine",
    "prUrlTemplate",
    "footerLinksRegexes",
    "subagentStatusLine",
    "enabledPlugins",
    "prependPlugins",
    "appendPlugins",
    "extraKnownMarketplaces",
    "additionalMarketplaces",
    "strictKnownMarketplaces",
    "allowedMarketplaces",
    "blockedMarketplaces",
    "disableCommandPluginSources",
    "disableSideloadFlags",
    "pluginSuggestionMarketplaces",
    "forceLoginMethod",
    "forceLoginGatewayUrl",
    "gatewayInternalNetworks",
    "parentSettingsBehavior",
    "managedSourcesBehavior",
    "forceLoginOrgUUID",
    "allowedProviders",
    "forceRemoteSettingsRefresh",
    "otelHeadersHelper",
    "outputStyle",
    "viewMode",
    "language",
    "skipWebFetchPreflight",
    "sandbox",
    "feedbackSurveyRate",
    "feedbackDrafts",
    "spinnerTipsEnabled",
    "clawdFileProgressEnabled",
    "spinnerVerbs",
    "spinnerTipsOverride",
    "syntaxHighlightingDisabled",
    "maxProseWidth",
    "spellcheck",
    "terminalTitleFromRename",
    "promptCacheTtl",
    "subagentPromptCacheTtl",
    "alwaysThinkingEnabled",
    "effortLevel",
    "maxEffortLevel",
    "modelSettings",
    "ultracode",
    "autoCompactWindow",
    "advisorModel",
    "fastMode",
    "fastModePerSessionOptIn",
    "promptSuggestionEnabled",
    "emojiCompletionEnabled",
    "awaySummaryEnabled",
    "responseComparisonEnabled",
    "showClearContextOnPlanAccept",
    "askUserQuestionTimeout",
    "dialogExpiry",
    "agent",
    "modelProposedGoals",
    "companyAnnouncements",
    "pluginConfigs",
    "remote",
    "autoUpdatesChannel",
    "minimumVersion",
    "requiredMinimumVersion",
    "requiredMaximumVersion",
    "plansDirectory",
    "tui",
    "voice",
    "channelsEnabled",
    "allowedChannelPlugins",
    "prefersReducedMotion",
    "timeFormat",
    "timeZone",
    "doneMeansMerged",
    "totalTokensReminder",
    "totalTokensReminderBudget",
    "totalTokensReminderAfterUserTurn",
    "autoMemoryEnabled",
    "autoMemoryDirectory",
    "autoDreamEnabled",
    "showThinkingSummaries",
    "skipDangerousModePermissionPrompt",
    "skipWorkflowUsageWarning",
    "disableAutoMode",
    "remoteTools",
    "sshConfigs",
    "sshHostCommands",
    "claudeMd",
    "claudeMdExcludes",
    "pluginTrustMessage",
    "theme",
    "editorMode",
    "keybindingFlavor",
    "vimInsertModeRemaps",
    "verbose",
    "preferredNotifChannel",
    "autoCompactEnabled",
    "precomputeCompactionEnabled",
    "precomputeArmTokens",
    "idleCompaction",
    "switchModelsOnFlag",
    "autoContinueAtUsageLimit",
    "autoScrollEnabled",
    "wheelScrollAccelerationEnabled",
    "fileCheckpointingEnabled",
    "showTurnDuration",
    "showMessageTimestamps",
    "terminalProgressBarEnabled",
    "todoFeatureEnabled",
    "teammateMode",
    "remoteControlAtStartup",
    "remoteControl",
    "isolatePeerMachines",
    "daemonColdStart",
    "crossSessionInbound",
    "autoUploadSessions",
    "inputNeededNotifEnabled",
    "agentPushNotifEnabled",
];

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
