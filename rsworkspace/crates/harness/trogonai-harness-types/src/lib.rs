//! Domain value objects shared by every runtime crate.
//!
//! This crate is the root of the dependency graph: it holds types only, has no
//! host or I/O dependencies, and compiles on every target including
//! `wasm32-unknown-unknown`. Behavior lives in the crates above it. The one
//! exception is [`local_time`], which asks the platform for the time zone.

#![forbid(unsafe_code)]

pub mod artifact_handle;
pub mod channel;
pub mod chat;
pub mod completion;
pub mod config;
pub mod context;
pub mod context_compaction;
pub mod context_note;
pub mod contract;
pub mod credential;
pub mod entry;
pub mod execution;
pub mod gateway;
pub mod gateway_routing;
pub mod generation;
pub mod hash;
pub mod hex;
pub mod history;
pub mod history_replay;
pub mod http;
pub mod id;
pub mod json;
pub mod layout;
pub mod lifecycle;
pub mod list_window;
pub mod local_time;
pub mod mcp;
pub mod memory;
pub mod notice;
pub mod operation;
pub mod output;
pub mod path;
pub mod process_token;
pub mod profile;
pub mod provider;
pub mod question;
pub mod reads;
pub mod recovery;
pub mod result;
pub mod secret;
pub mod service_tier;
pub mod span;
pub mod subagent;
pub mod subagent_status;
pub mod time;
pub mod tokens;
pub mod tool_image;
pub mod tool_output;
pub mod turn;
pub mod web;

pub use artifact_handle::{ArtifactHandle, InvalidArtifactHandle};
pub use channel::Channel;
pub use chat::{
    ArgumentDiagnostic, ArgumentFailure, ArgumentIntegrity, ImageAttachment, Message, Provenance,
    ProviderPromptProjection, ProviderReplay, ResolvedDisplayTarget, Role, ToolCall, ToolStatus,
    project_provider_replay,
};
pub use completion::{
    Disposition, IdentityField, MAX_CONVERSATION_IDENTITY_BYTES, ProviderCompletion, ProviderFailureCause,
    ProviderResultFault, ToolAdmission, ToolIdentityFault,
};
pub use config::{
    BackendKind, ConversationLanguage, ConversationLanguageError, CredentialSource, EffortName, EffortNameError,
    ReasoningEffort, UnknownBackend, UnknownCredentialSource,
};
pub use context::{OMISSION_SOURCE_PREFIX_BYTES, PROJECT_INSTRUCTION_GUIDANCE, RETAINED_OMISSION_COUNT, TRIM_CHARS};
pub use context_compaction::ContextCompactionTransactionResult;
pub use credential::{
    CredentialAccountId, CredentialAccountIdError, CredentialAuthority, CredentialLease, DirectCredentialLease,
    ProviderAuthorization,
};
pub use entry::{FileBirth, FileIdentity, FileKind};
pub use execution::{
    ExecutionMemory, FileEvidence, FileEvidenceAction, PersistedSteering, SteeringDelivery, ToolExecutionStep, UserTurn,
};
pub use gateway::{Credential, GatewayIdError, GatewayTimestamp, GenerationId, TeamId};
pub use gateway_routing::{
    ConflictingStrictness, InvalidServedProvider, MAX_PROVIDER_ORDER_ENTRIES, MAX_PROVIDER_SLUG_BYTES,
    MAX_SERVED_PROVIDER_BYTES, ProviderOrder, ProviderRouting, ProviderRoutingError, ProviderRoutingOverride,
    ProviderSlug, ServedProvider,
};
pub use generation::{GenerationCost, GenerationRecord, GenerationTokens, NotACost};
pub use hash::{ContentHash, ContentHasher};
pub use history::{
    AssistantHistoryTurn, CONTEXT_HANDOFF_CLOSE, CONTEXT_HANDOFF_OPEN, CancellationOrigin, CompactedSummaryHistoryTurn,
    ContextHistoryCut, FinishedPrompt, FinishedPromptProjection, HistoryTurn, InterruptedChatProjection,
    InterruptedHistoryTurn, InterruptedTerminalReason, LegacyCompactedContext, RetainedCompactionWindow,
    RetainedHistoryTail, SnapshotFileHold, latest_compaction_count,
};
pub use history_replay::{
    LegacyTerminalCall, SubagentHistoryCall, SubagentHistoryDisposition, find_legacy_terminal_call,
    find_subagent_history_call,
};
pub use http::HttpStatus;
pub use id::{
    CheckId, ContractId, IdError, McpServerId, MessageId, ModeId, ModelId, SessionId, SubagentId, ToolCallId, ToolName,
    WorkId,
};
pub use layout::{Layout, Metrics, TerminalTooSmall, WindowSize};
pub use lifecycle::{
    ActivityCounts, FinishedTool, ProvisionalTool, StartedTool, StepId, StreamState, ToolActivityKind,
    ToolLifecycleEvent, ToolLifecycleId, ToolOutcome, ToolOutcomeKind, ToolPresentationGroupId, TurnFinished, TurnId,
    TurnPhase, TurnPhaseUpdate, TurnPresentationOutcome, TurnTiming,
};
pub use list_window::{MenuStep, Window};
pub use mcp::McpToolBinding;
pub use memory::{StoredOutput, ToolResultMemory};
pub use notice::{NoticeTone, NoticeVisibility, SemanticNotice, context_notice_body, notice_glyph};
pub use operation::{BoundOperationId, OperationAuthority, OperationSource};
pub use output::OutputFormat;
pub use path::{MAX_FILE_TARGET_COMPONENTS, PathSpan};
pub use process_token::{InvalidProcessInstanceToken, ProcessInstanceToken};
pub use profile::Profile;
pub use provider::{
    ByProvider, ConfiguredProviderIdentity, ConfiguredProviderRegistry, InvalidProviderBinding,
    MAX_CONFIGURED_PROVIDER_ID_BYTES, NameKey, ProviderBindError, ProviderId, ProviderSelection,
    ValidateConfiguredIdError,
};
pub use reads::ReadCoverage;
pub use recovery::{
    Attempt, ModelFailureDiagnostic, ModelRecoveryAction, ModelRecoveryCause, ModelRecoveryRequiredAction,
    PausedRecovery, RetryProgress, RouteRecoveryKind, RouteRecoveryStatus, RouteRecoveryStatusTone,
    RouteRecoveryUnsafeReason,
};
pub use result::{
    CancelledCommandPresentation, CommandOutputReplay, CommandOutputReplayDescriptor, CommandProcessOutcome,
    CommittedFileContent, CommittedFileKind, CommittedFileLine, CommittedFileLineKind, CommittedFilePresentation,
    CommittedFileReview, CommittedFileReviewArtifact, DiffStat, PersistedToolResult, PersistedToolStatus,
    TerminalActionPresentation, TerminalFailurePresentation, TerminalReturnPresentation, ToolDeferral,
    ToolResultArtifacts,
};
pub use secret::SecretBytes;
pub use span::{ImageId, ImageToken, Span};
pub use subagent_status::{SubagentStatus, SubagentStatusRenderer};
pub use time::{CivilDate, CivilDateTime, UnixMillis, format_session_list_timestamp};
pub use tokens::{Billing, ProviderUsage, TokenCount, ToolUsage, TurnTokens};
pub use tool_image::ToolImage;
pub use tool_output::{
    DurableToolText, InvalidToolLinkedResource, ToolLinkedResource, ToolOutputFragment, ToolReadEvidence,
    ToolResultObservations, ToolStructuredContent, ToolTextTooLarge,
};
pub use turn::{FinishReason, TurnSummary};
pub use web::{
    FetchedUrl, WebFetchArtifactState, WebFetchCompletion, WebFetchProgress, WebSearchCompletion, WebSearchProgress,
};

pub mod external_observation;

pub use provider::HostProviderBinding;
