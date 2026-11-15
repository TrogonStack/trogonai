//! Each Python module of 0.2.181 imports first-party names from its siblings. The tables below pair every such
//! name with the item of this crate that answers it. A `use` fails to compile when the item is not reachable
//! from the crate root, and the table is checked against what the tables must cover.

macro_rules! reachable {
    ($module:ident { $($python:literal => $rust:ident),* $(,)? }) => {
        mod $module {
            $(#[allow(unused_imports)] use trogon_claude_agent_sdk::$rust as _;)*
            pub const PAIRS: &[(&str, &str)] = &[$(($python, stringify!($rust))),*];
        }
    };
}

reachable!(errors {
    "ClaudeSDKError" => Error,
    "CLIConnectionError" => Error,
    "CLIJSONDecodeError" => JsonDecodeError,
    "CLINotFoundError" => CliNotFoundError,
    "ProcessError" => Error,
    "ResultError" => ResultError,
});

reachable!(package {
    "build_tool_server" => SdkMcpServer,
    "import_session_to_store" => import_session_to_store,
    "ForkSessionResult" => ForkSessionResult,
    "delete_session" => delete_session,
    "delete_session_via_store" => delete_session_via_store,
    "fork_session" => fork_session,
    "fork_session_via_store" => fork_session_via_store,
    "rename_session" => rename_session,
    "rename_session_via_store" => rename_session_via_store,
    "tag_session" => tag_session,
    "tag_session_via_store" => tag_session_via_store,
    "InMemorySessionStore" => InMemorySessionStore,
    "project_key_for_directory" => project_key_for_directory,
    "fold_session_summary" => fold_session_summary,
    "get_session_info" => get_session_info,
    "get_session_info_from_store" => get_session_info_from_store,
    "get_session_messages" => get_session_messages,
    "get_session_messages_from_store" => get_session_messages_from_store,
    "get_subagent_messages" => get_subagent_messages,
    "get_subagent_messages_from_store" => get_subagent_messages_from_store,
    "list_sessions" => list_sessions,
    "list_sessions_from_store" => list_sessions_from_store,
    "list_subagents" => list_subagents,
    "list_subagents_from_store" => list_subagents_from_store,
    "Transport" => Transport,
    "__version__" => SDK_VERSION,
    "ClaudeSDKClient" => ClaudeSdkClient,
    "query" => query,
    "AgentDefinition" => AgentDefinition,
    "AssistantMessage" => AssistantMessage,
    "ClaudeAgentOptions" => ClaudeAgentOptions,
    "ContentBlock" => ContentBlock,
    "ContextUsageCategory" => ContextUsageCategory,
    "ContextUsageResponse" => ContextUsageResponse,
    "ConversationResetMessage" => ConversationResetMessage,
    "DeferredToolUse" => DeferredToolUse,
    "EffortLevel" => EffortLevel,
    "HookEventMessage" => HookEventMessage,
    "McpStatusResponse" => McpStatusResponse,
    "Message" => Message,
    "MessageOrigin" => MessageOrigin,
    "MirrorErrorMessage" => MirrorErrorMessage,
    "PermissionMode" => PermissionMode,
    "PermissionUpdate" => PermissionUpdate,
    "RateLimitEvent" => RateLimitEvent,
    "RateLimitInfo" => RateLimitInfo,
    "ResultMessage" => ResultMessage,
    "SandboxIgnoreViolations" => SandboxIgnoreViolations,
    "SandboxNetworkConfig" => SandboxNetworkConfig,
    "SandboxSettings" => SandboxSettings,
    "SdkBeta" => SdkBeta,
    "SDKSessionInfo" => SdkSessionInfo,
    "SessionKey" => SessionKey,
    "SessionMessage" => SessionMessage,
    "SessionStore" => SessionStore,
    "SessionStoreListEntry" => SessionListEntry,
    "SessionSummaryEntry" => SessionSummaryEntry,
    "SettingSource" => SettingSource,
    "StreamEvent" => StreamEvent,
    "SystemMessage" => SystemMessage,
    "TaskBudget" => TaskBudget,
    "TaskNotificationMessage" => TaskNotificationMessage,
    "TaskProgressMessage" => TaskProgressMessage,
    "TaskStartedMessage" => TaskStartedMessage,
    "TaskUpdatedMessage" => TaskUpdatedMessage,
    "ThinkingConfig" => ThinkingConfig,
    "UserMessage" => UserMessage,
});

reachable!(session_modules {
    "SessionStoreEntry" => SessionEntry,
    "SessionKey" => SessionKey,
    "SDKSessionInfo" => SdkSessionInfo,
    "SessionMessage" => SessionMessage,
    "SessionStore" => SessionStore,
    "SessionSummaryEntry" => SessionSummaryEntry,
    "fold_session_summary" => fold_session_summary,
    "project_key_for_directory" => project_key_for_directory,
});

reachable!(query_and_client_modules {
    "Transport" => Transport,
    "ClaudeAgentOptions" => ClaudeAgentOptions,
    "Message" => Message,
    "CLIConnectionError" => Error,
    "ContextUsageResponse" => ContextUsageResponse,
    "McpStatusResponse" => McpStatusResponse,
    "PermissionMode" => PermissionMode,
    "ResultMessage" => ResultMessage,
});

fn assert_pairs(pairs: &[(&str, &str)], expected_count: usize) {
    assert_eq!(pairs.len(), expected_count);
    for (python, rust) in pairs {
        assert!(!python.is_empty() && !rust.is_empty());
    }
}

#[test]
fn errors_module_imports_reach_the_error_types() {
    assert_pairs(errors::PAIRS, 6);
}

#[test]
fn package_imports_reach_the_crate_root() {
    assert_pairs(package::PAIRS, 67);
}

#[test]
fn session_module_imports_reach_the_session_types() {
    assert_pairs(session_modules::PAIRS, 8);
}

#[test]
fn query_and_client_module_imports_reach_the_runtime_types() {
    assert_pairs(query_and_client_modules::PAIRS, 8);
}
