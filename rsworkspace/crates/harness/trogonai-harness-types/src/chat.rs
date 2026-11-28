//! Conversation messages exchanged with a provider.

use serde::{Deserialize, Serialize};

use crate::memory::ToolResultMemory;
use crate::provider::ProviderSelection;
use crate::{ToolCallId, ToolName, UnixMillis};

/// An image handed to the model alongside text.
///
/// Field order is the order the session file writes, and `snapshot_path` and
/// `snapshot_sha256` are set together or not at all: a snapshot without its
/// hash cannot be trusted for replay, and a hash without its file cannot be
/// read back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAttachment {
    /// Position in the turn's catalog, and what a placeholder names.
    ///
    /// Zero until the turn assigns one; catalog ids start at one.
    #[serde(default)]
    pub id: usize,
    /// Path the image was read from.
    pub path: String,
    /// IANA media type, such as `image/png`.
    pub media_type: String,
    /// Where the image was copied to when the turn was snapshotted for replay.
    #[serde(default)]
    pub snapshot_path: Option<String>,
    /// Content hash of the snapshot.
    #[serde(default)]
    pub snapshot_sha256: Option<String>,
}

/// Whether a tool call's serialized arguments can be given to the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgumentIntegrity {
    /// Arguments parsed.
    Valid,
    /// Arguments are not a single complete JSON document, or repeat an object key.
    MalformedJson,
    /// Arguments are a complete JSON document, but not an object.
    ///
    /// Only a call the runtime runs itself is held to this. A tool takes named
    /// arguments, so a bare array or string parses and still cannot be handed
    /// to one.
    NonObjectJson,
}

impl ArgumentIntegrity {
    /// Classifies serialized arguments.
    ///
    /// See [`crate::json`] for why the last one is refused rather than resolved.
    #[must_use]
    pub fn classify(serialized: &str) -> Self {
        if crate::json::is_strict_document(serialized) {
            Self::Valid
        } else {
            Self::MalformedJson
        }
    }

    /// Classifies arguments that will be handed to a tool the runtime runs itself.
    ///
    /// Stricter than [`Self::classify`] by one rule: the root must be an
    /// object. A provider that executed the call already accepted whatever it
    /// was given, so nothing is gained by refusing it after the fact; a call the runtime
    /// is about to dispatch has parameters to bind, and there is nothing to
    /// bind them from.
    #[must_use]
    pub fn classify_function_input(serialized: &str) -> Self {
        match Self::classify(serialized) {
            Self::Valid => {
                if serialized.trim_start().starts_with('{') {
                    Self::Valid
                } else {
                    Self::NonObjectJson
                }
            }
            other => other,
        }
    }
}

/// Why raw tool arguments were refused as malformed JSON.
///
/// Carries positions only. The rejected bytes never leave the parse step, so a
/// diagnostic can be shown to the model and written to a transcript without
/// repeating whatever the arguments held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgumentDiagnostic {
    failure: ArgumentFailure,
    received_bytes: usize,
}

/// The way malformed arguments failed to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentFailure {
    /// The input ended before the JSON value was complete.
    Truncated,
    /// Parsing stopped at the byte at `offset`.
    SyntaxError {
        /// Byte offset where parsing stopped.
        offset: usize,
    },
    /// The syntax was complete, but the value was refused, such as an object
    /// that repeats a key.
    RejectedValue,
}

impl ArgumentFailure {
    /// The stable token a reader compares against.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Truncated => "truncated",
            Self::SyntaxError { .. } => "syntax_error",
            Self::RejectedValue => "rejected_value",
        }
    }
}

impl ArgumentDiagnostic {
    /// Diagnoses raw arguments, or `None` when they are one strict document.
    #[must_use]
    pub fn of(raw: &str) -> Option<Self> {
        let failure = match crate::json::strict_document_fault(raw)? {
            crate::json::DocumentFault::Truncated => ArgumentFailure::Truncated,
            crate::json::DocumentFault::Syntax { offset } => ArgumentFailure::SyntaxError { offset },
            crate::json::DocumentFault::RejectedValue => ArgumentFailure::RejectedValue,
        };
        Some(Self {
            failure,
            received_bytes: raw.len(),
        })
    }

    /// How parsing failed.
    #[must_use]
    pub const fn failure(&self) -> ArgumentFailure {
        self.failure
    }

    /// How many bytes of arguments arrived.
    #[must_use]
    pub const fn received_bytes(&self) -> usize {
        self.received_bytes
    }

    /// Where parsing stopped, when one byte is at fault or the input ran out.
    #[must_use]
    pub const fn error_offset(&self) -> Option<usize> {
        match self.failure {
            ArgumentFailure::Truncated => Some(self.received_bytes),
            ArgumentFailure::SyntaxError { offset } => Some(offset),
            ArgumentFailure::RejectedValue => None,
        }
    }
}

/// Who executed a tool call, and what came back if it was not trogonai.
///
/// The result travels with the provenance rather than beside it. In the
/// reference these are two fields, `provenance` and `provider_result`, and
/// `authoritativeToolAdmission` exists partly to reject the combination where a
/// provider-executed call arrived without one. Here that combination does not
/// typecheck, so the rejection moves to the decoder that reads the wire, which
/// is the only place that can still see it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// The runtime dispatched the call locally.
    #[default]
    Local,
    /// The provider ran the tool server-side.
    ProviderExecuted {
        /// The result it returned.
        result: String,
    },
}

/// What a row says a call acts on, once the runtime has worked it out.
///
/// A read of saved output names a handle the person never sees. The runtime
/// finds the call that saved it, and this carries that call's label to the
/// surfaces that draw the row. It is never sent to a provider or written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDisplayTarget(String);

impl ResolvedDisplayTarget {
    /// Wraps text that is already safe to draw.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text a row shows.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A tool invocation requested by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned call identity.
    pub id: ToolCallId,
    /// Requested tool.
    pub name: ToolName,
    /// Raw serialized arguments, kept verbatim for replay fidelity.
    pub arguments_json: String,
    /// Who ran the call, carrying the upstream result when it was not trogonai.
    pub provenance: Provenance,
    /// The id this call was announced under, when the provider renamed it.
    ///
    /// A stream can open a tool input under one id and then close the call
    /// under another. Anything that already showed the call, or already wrote
    /// it down, knows it by the first id, so the first id travels with the
    /// call rather than being dropped once the second one arrives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provisional_id: Option<ToolCallId>,
    /// What the row names the call after, when the runtime resolved it.
    #[serde(skip)]
    pub resolved_display_target: Option<ResolvedDisplayTarget>,
}

impl ToolCall {
    /// Builds a call the runtime will dispatch itself.
    #[must_use]
    pub fn local(id: ToolCallId, name: ToolName, arguments_json: String) -> Self {
        Self {
            id,
            name,
            arguments_json,
            provenance: Provenance::Local,
            provisional_id: None,
            resolved_display_target: None,
        }
    }

    /// Names what the row acts on.
    #[must_use]
    pub fn displayed_as(mut self, target: ResolvedDisplayTarget) -> Self {
        self.resolved_display_target = Some(target);
        self
    }

    /// Records the id this call was first announced under.
    #[must_use]
    pub fn announced_as(mut self, provisional_id: ToolCallId) -> Self {
        self.provisional_id = Some(provisional_id);
        self
    }

    /// Builds a call the provider already ran, carrying its result.
    #[must_use]
    pub fn provider_executed(id: ToolCallId, name: ToolName, arguments_json: String, result: String) -> Self {
        Self {
            id,
            name,
            arguments_json,
            provenance: Provenance::ProviderExecuted { result },
            provisional_id: None,
            resolved_display_target: None,
        }
    }

    /// Whether the arguments are one complete JSON document.
    ///
    /// Classified on demand rather than stored. A stored verdict is a second
    /// source of truth about the same string, and the two drift the moment
    /// anyone assigns to `arguments_json`.
    #[must_use]
    pub fn argument_integrity(&self) -> ArgumentIntegrity {
        match self.provenance {
            Provenance::ProviderExecuted { .. } => ArgumentIntegrity::classify(&self.arguments_json),
            Provenance::Local => ArgumentIntegrity::classify_function_input(&self.arguments_json),
        }
    }

    /// Whether the arguments can be handed to a tool as they stand.
    #[must_use]
    pub fn arguments_are_wellformed(&self) -> bool {
        self.argument_integrity() == ArgumentIntegrity::Valid
    }

    /// The result the provider computed, for a provider-executed call.
    #[must_use]
    pub fn provider_result(&self) -> Option<&str> {
        match &self.provenance {
            Provenance::Local => None,
            Provenance::ProviderExecuted { result } => Some(result),
        }
    }
}

/// Outcome recorded for a completed tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// The tool ran and produced a result.
    Success,
    /// The tool failed.
    Error,
    /// The user or the runtime cancelled the call.
    Cancelled,
    /// A permission rule denied the call.
    Denied,
}

/// One turn in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// Host-admitted external data, distinct from instructions and model commands.
    ExternalObservation {
        /// Exact untrusted producer data and provenance.
        observation: crate::external_observation::ExternalObservation,
    },
    /// Durable capture metadata, removed before provider serialization.
    ContextSnapshot {
        /// The instruction context frozen at first admission.
        snapshot: crate::context_note::CapturedRuntimeContext,
    },
    /// Runtime-authored guidance projected onto a provider carrier.
    ContextNote {
        /// Typed provenance and admitted guidance.
        note: crate::context_note::ContextNote,
    },
    /// System instructions.
    System {
        /// Instruction text.
        content: String,
    },
    /// A user prompt.
    User {
        /// Prompt text.
        content: String,
        /// Attached images.
        #[serde(default)]
        images: Vec<ImageAttachment>,
    },
    /// What the user said when answering a permission prompt.
    ///
    /// It is a message the person wrote about one tool call rather than a prompt, so it is its
    /// own variant: nothing else can be flagged as feedback, and an ordinary prompt cannot name
    /// a call.
    PermissionFeedback {
        /// What the person said.
        content: String,
        /// The call the feedback was about, absent when it followed the last
        /// result without naming it.
        #[serde(default)]
        tool_call_id: Option<ToolCallId>,
    },
    /// A model response, optionally requesting tools.
    Assistant {
        /// Response text, absent when the model only requested tools.
        content: Option<String>,
        /// Tools the model asked to run.
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        /// The provider's own rendering of this turn, when it gave one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_replay: Option<ProviderReplay>,
        /// Whether this turn answered on its own rather than opening tool work.
        #[serde(default)]
        standalone_response: bool,
    },
    /// The result of one tool call, fed back to the model.
    ToolResult {
        /// Call this result answers.
        tool_call_id: ToolCallId,
        /// Tool that produced it.
        tool_name: ToolName,
        /// Serialized result content.
        content: String,
        /// How the call ended.
        status: ToolStatus,
        /// What is known about the result beyond the text the model was shown.
        #[serde(default)]
        memory: Option<ToolResultMemory>,
        /// When the live result was accepted, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_at: Option<UnixMillis>,
    },
}

/// The role a provider is told a message was written in.
///
/// [`Message::PermissionFeedback`] answers [`Self::User`]: the distinction it
/// draws is the runtime's, and a provider has no field for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Instructions that frame the conversation.
    System,
    /// The person.
    User,
    /// The model.
    Assistant,
    /// A tool result fed back to the model.
    Tool,
}

impl Role {
    /// The wire spelling, which is also what a trace line prints.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

impl Message {
    /// Convenience constructor for a plain user prompt.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
            images: Vec::new(),
        }
    }

    /// Convenience constructor for system instructions.
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    /// A model response, with the tools it asked to run.
    #[must_use]
    pub fn assistant(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self::Assistant {
            content,
            tool_calls,
            provider_replay: None,
            standalone_response: false,
        }
    }

    /// One tool result on its way back to the model.
    #[must_use]
    pub fn tool_result(
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        content: impl Into<String>,
        status: ToolStatus,
    ) -> Self {
        Self::ToolResult {
            tool_call_id,
            tool_name,
            content: content.into(),
            status,
            memory: None,
            created_at: None,
        }
    }

    /// What the person said when answering a permission prompt.
    #[must_use]
    pub fn permission_feedback(content: impl Into<String>) -> Self {
        Self::PermissionFeedback {
            content: content.into(),
            tool_call_id: None,
        }
    }

    /// The same message, said about one named call rather than the last result.
    #[must_use]
    pub fn about_call(mut self, call: ToolCallId) -> Self {
        if let Self::PermissionFeedback { tool_call_id, .. } = &mut self {
            *tool_call_id = Some(call);
        }
        self
    }

    /// The same result, remembering what its text no longer states.
    #[must_use]
    pub fn remembering(mut self, remembered: ToolResultMemory) -> Self {
        if let Self::ToolResult { memory, .. } = &mut self {
            *memory = Some(remembered);
        }
        self
    }

    /// Keeps a usable result birth time when projecting persisted history.
    #[must_use]
    pub fn created_at(mut self, at: Option<UnixMillis>) -> Self {
        if let Self::ToolResult { created_at, .. } = &mut self {
            *created_at = at.filter(|stamp| stamp.as_millis() > 0);
        }
        self
    }

    /// The role a provider is told this message was written in.
    #[must_use]
    pub const fn role(&self) -> Role {
        match self {
            Self::System { .. } => Role::System,
            Self::User { .. }
            | Self::PermissionFeedback { .. }
            | Self::ContextNote { .. }
            | Self::ContextSnapshot { .. }
            | Self::ExternalObservation { .. } => Role::User,
            Self::Assistant { .. } => Role::Assistant,
            Self::ToolResult { .. } => Role::Tool,
        }
    }

    /// The text of this message, absent only for a model turn that said nothing.
    #[must_use]
    pub fn content(&self) -> Option<&str> {
        match self {
            Self::System { content, .. }
            | Self::User { content, .. }
            | Self::PermissionFeedback { content, .. }
            | Self::ToolResult { content, .. } => Some(content),
            Self::Assistant { content, .. } => content.as_deref(),
            Self::ContextNote { note } => Some(&note.text),
            Self::ExternalObservation { observation } => Some(&observation.text),
            Self::ContextSnapshot { .. } => None,
        }
    }

    /// Whether this is the person answering a permission prompt.
    #[must_use]
    pub const fn is_permission_feedback(&self) -> bool {
        matches!(self, Self::PermissionFeedback { .. })
    }

    /// The call this message is about, for the two kinds that name one.
    #[must_use]
    pub const fn tool_call_id(&self) -> Option<&ToolCallId> {
        match self {
            Self::ToolResult { tool_call_id, .. } => Some(tool_call_id),
            Self::PermissionFeedback { tool_call_id, .. } => tool_call_id.as_ref(),
            _ => None,
        }
    }

    /// What is remembered about this tool result beyond its text.
    #[must_use]
    pub fn memory(&self) -> Option<&ToolResultMemory> {
        match self {
            Self::ToolResult { memory, .. } => memory.as_ref(),
            _ => None,
        }
    }

    /// Tool calls this message requested, empty for every other role.
    #[must_use]
    pub fn tool_calls(&self) -> &[ToolCall] {
        match self {
            Self::Assistant { tool_calls, .. } => tool_calls,
            _ => &[],
        }
    }

    /// The provider's own rendering of this turn, when it gave one.
    #[must_use]
    pub fn provider_replay(&self) -> Option<&ProviderReplay> {
        match self {
            Self::Assistant { provider_replay, .. } => provider_replay.as_ref(),
            _ => None,
        }
    }

    /// Whether this turn is worth keeping once its tool work is accounted for.
    ///
    /// A model turn that asked for no tools is ordinarily an artefact of how
    /// the step was cut, so it is dropped. These two say otherwise: one because
    /// the turn answered on its own, the other because the provider's own
    /// rendering of it would be lost with it.
    #[must_use]
    pub fn stands_on_its_own(&self) -> bool {
        match self {
            Self::Assistant {
                provider_replay,
                standalone_response,
                ..
            } => *standalone_response || provider_replay.is_some(),
            _ => false,
        }
    }
}

/// The provider's own rendering of one assistant turn, kept for replay.
///
/// Bound to the route that wrote it: parts are provider-shaped, so a turn
/// replayed to a different provider or model would be handing it a document it
/// has no grammar for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderReplay {
    /// The route that produced these parts.
    pub source: ProviderSelection,
    /// The provider's parts, verbatim.
    pub parts_json: String,
}

impl ProviderReplay {
    /// The largest replay worth carrying.
    pub const MAX_BYTES: usize = 4 * 1024 * 1024;

    /// Sets one provider message apart from the next in assistant text.
    ///
    /// Replayed text spans leave it out, so each message returns to its
    /// provider as it was said.
    pub const MESSAGE_SEPARATOR: &'static str = "\n\n";

    /// Whether these parts came from the route about to be asked.
    #[must_use]
    pub fn matches(&self, source: &ProviderSelection) -> bool {
        self.source.model == source.model && self.source.provider.same_authority(source.provider)
    }
}

/// The same history with replay from other routes dropped.
///
/// Returns nothing when every replay already came from `source`, which is the common case and
/// the one where a copy would buy nothing: the caller sends what it already holds.
#[must_use]
pub fn project_provider_replay(messages: &[Message], source: &ProviderSelection) -> Option<Vec<Message>> {
    let mut projected: Option<Vec<Message>> = None;
    for (index, message) in messages.iter().enumerate() {
        let Some(replay) = message.provider_replay() else {
            continue;
        };
        if replay.matches(source) {
            continue;
        }
        let owned = projected.get_or_insert_with(|| messages.to_vec());
        if let Message::Assistant { provider_replay, .. } = &mut owned[index] {
            *provider_replay = None;
        }
    }
    projected
}

/// Instructions and messages a provider request is built from.
///
/// The index names where the current user message sits inside [`Self::messages`], which moves
/// when compaction replaces durable history with a handoff block.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderPromptProjection {
    /// System messages a provider may cache across steps.
    pub instructions: Vec<Message>,
    /// Conversation messages in provider order.
    pub messages: Vec<Message>,
    /// Index of the current user message inside [`Self::messages`].
    pub current_user_index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ModelId;
    use crate::provider::ProviderId;

    fn call(args: &str) -> ToolCall {
        ToolCall::local(
            ToolCallId::new("c1").expect("id"),
            ToolName::new("read").expect("name"),
            args.to_string(),
        )
    }

    #[test]
    fn a_diagnostic_reports_positions_for_each_failure() {
        let cut = "{\"request\":{\"task\":\"TROGONAI_REJECTED_SOURCE_SENTINEL and more";
        let truncated = ArgumentDiagnostic::of(cut).expect("malformed");
        assert_eq!(truncated.failure(), ArgumentFailure::Truncated);
        assert_eq!(truncated.received_bytes(), cut.len());
        assert_eq!(truncated.error_offset(), Some(cut.len()));
        assert_eq!(truncated.failure().as_str(), "truncated");

        let trailing = "{\"path\":\"a\",}";
        let syntax = ArgumentDiagnostic::of(trailing).expect("malformed");
        assert_eq!(syntax.failure().as_str(), "syntax_error");
        assert_eq!(syntax.error_offset(), Some(trailing.len() - 1));

        let repeated = ArgumentDiagnostic::of("{\"a\":1,\"a\":2}").expect("malformed");
        assert_eq!(repeated.failure(), ArgumentFailure::RejectedValue);
        assert_eq!(repeated.error_offset(), None);

        assert_eq!(ArgumentDiagnostic::of("{\"a\":1}"), None);
        assert_eq!(
            ArgumentDiagnostic::of("[1]"),
            None,
            "a complete non-object document is not malformed"
        );
    }

    #[test]
    fn classifies_malformed_arguments() {
        assert_eq!(
            call("{\"path\":").argument_integrity(),
            ArgumentIntegrity::MalformedJson
        );
        assert_eq!(call("{\"path\":\"a\"}").argument_integrity(), ArgumentIntegrity::Valid);
    }

    #[test]
    fn a_call_trogonai_runs_itself_needs_named_arguments() {
        for args in ["[1,2]", "\"text\"", "42", "null", "true"] {
            assert_eq!(
                call(args).argument_integrity(),
                ArgumentIntegrity::NonObjectJson,
                "{args}"
            );
        }
        assert_eq!(
            call("  \n {\"path\":\"a\"} ").argument_integrity(),
            ArgumentIntegrity::Valid,
            "leading whitespace does not hide the object"
        );
    }

    #[test]
    fn a_call_the_provider_already_ran_is_only_held_to_being_a_document() {
        let executed = ToolCall::provider_executed(
            ToolCallId::new("c1").expect("id"),
            ToolName::new("web_search").expect("name"),
            "[1,2]".to_owned(),
            "ok".to_owned(),
        );
        assert_eq!(executed.argument_integrity(), ArgumentIntegrity::Valid);
    }

    fn route(provider: ProviderId, model: &str) -> ProviderSelection {
        ProviderSelection {
            provider,
            model: ModelId::new(model).expect("model"),
        }
    }

    fn replayed(source: &ProviderSelection) -> Message {
        Message::Assistant {
            content: Some("answer".to_owned()),
            tool_calls: Vec::new(),
            provider_replay: Some(ProviderReplay {
                source: source.clone(),
                parts_json: "[]".to_owned(),
            }),
            standalone_response: false,
        }
    }

    #[test]
    fn a_replay_is_kept_only_for_the_route_that_wrote_it() {
        let source = route(ProviderId::Gateway, "model");
        let messages = vec![replayed(&source)];
        assert!(
            project_provider_replay(&messages, &source).is_none(),
            "a history the route already wrote needs no projection"
        );

        for other in [
            route(ProviderId::Codex, "model"),
            route(ProviderId::Grok, "model"),
            route(ProviderId::Gateway, "different"),
        ] {
            let projected = project_provider_replay(&messages, &other).expect("a projection");
            assert_eq!(projected[0].provider_replay(), None);
            assert_eq!(
                projected[0].content(),
                Some("answer"),
                "dropping the replay leaves the prose the model wrote"
            );
            assert!(
                messages[0].provider_replay().is_some(),
                "the history the projection was read from is left alone"
            );
            assert!(
                project_provider_replay(&projected, &other).is_none(),
                "projecting twice finds nothing left to drop"
            );
        }
    }

    #[test]
    fn a_model_turn_without_tool_work_stands_on_its_own_only_when_it_says_so() {
        let source = route(ProviderId::Gateway, "model");
        assert!(!Message::assistant(Some("done".into()), Vec::new()).stands_on_its_own());
        assert!(replayed(&source).stands_on_its_own());
        assert!(
            Message::Assistant {
                content: Some("done".to_owned()),
                tool_calls: Vec::new(),
                provider_replay: None,
                standalone_response: true,
            }
            .stands_on_its_own()
        );
        assert!(
            !Message::user("hi").stands_on_its_own(),
            "only a model turn is ever cut for having opened no tool work"
        );
    }

    #[test]
    fn tool_calls_are_empty_off_the_assistant_role() {
        assert!(Message::user("hi").tool_calls().is_empty());
    }

    #[test]
    fn round_trips_through_json() {
        let msg = Message::tool_result(
            ToolCallId::new("c1").expect("id"),
            ToolName::new("read").expect("name"),
            "ok",
            ToolStatus::Success,
        );
        let encoded = serde_json::to_string(&msg).expect("encode");
        assert_eq!(serde_json::from_str::<Message>(&encoded).expect("decode"), msg);
    }

    #[test]
    fn permission_feedback_reaches_the_provider_as_a_user_message() {
        let feedback = Message::permission_feedback("run it, but read the file first");
        assert_eq!(feedback.role(), Role::User);
        assert!(feedback.is_permission_feedback());
        assert!(
            !Message::user("run it, but read the file first").is_permission_feedback(),
            "a prompt that reads like feedback is still a prompt"
        );
    }

    #[test]
    fn feedback_that_names_no_call_answers_for_the_last_result() {
        let unnamed = Message::permission_feedback("go ahead");
        assert_eq!(unnamed.tool_call_id(), None);

        let named = unnamed.about_call(ToolCallId::new("c7").expect("id"));
        assert_eq!(
            named.tool_call_id().map(ToolCallId::as_str),
            Some("c7"),
            "naming a call is what separates feedback from the result it follows"
        );
    }

    #[test]
    fn a_prompt_cannot_name_a_call_and_a_result_always_names_one() {
        assert_eq!(Message::user("hello").tool_call_id(), None);
        assert_eq!(Message::system("rules").tool_call_id(), None);
        assert_eq!(
            Message::tool_result(
                ToolCallId::new("c1").expect("id"),
                ToolName::new("read").expect("name"),
                "ok",
                ToolStatus::Success,
            )
            .tool_call_id()
            .map(ToolCallId::as_str),
            Some("c1")
        );
    }

    #[test]
    fn provider_prompt_projection_records_where_the_current_user_message_sits() {
        let projection = ProviderPromptProjection {
            instructions: vec![Message::system("stable")],
            messages: vec![Message::user("history"), Message::user("pending")],
            current_user_index: 1,
        };
        assert_eq!(projection.messages.len(), 2);
        assert_eq!(projection.current_user_index, 1);
    }

    #[test]
    fn a_clamped_result_looks_whole_until_its_memory_is_read() {
        let plain = Message::tool_result(
            ToolCallId::new("c1").expect("id"),
            ToolName::new("read").expect("name"),
            "first 64 KB",
            ToolStatus::Success,
        );
        assert_eq!(plain.memory(), None);

        let remembered = plain.remembering(ToolResultMemory::inline(200_000, 65_536, true));
        assert!(
            remembered.memory().expect("a memory").was_truncated(),
            "the text alone cannot say the rest was withheld"
        );
    }

    #[test]
    fn only_a_model_turn_can_say_nothing() {
        assert_eq!(Message::user("hi").content(), Some("hi"));
        assert_eq!(Message::assistant(None, Vec::new()).content(), None);
        assert_eq!(
            Message::assistant(Some("done".into()), Vec::new()).content(),
            Some("done")
        );
    }

    #[test]
    fn every_role_the_provider_knows_has_a_message_that_answers_it() {
        assert_eq!(Message::system("rules").role(), Role::System);
        assert_eq!(Message::user("hi").role(), Role::User);
        assert_eq!(Message::assistant(None, vec![call("{}")]).role(), Role::Assistant);
        assert_eq!(
            Message::tool_result(
                ToolCallId::new("c1").expect("id"),
                ToolName::new("read").expect("name"),
                "ok",
                ToolStatus::Success,
            )
            .role(),
            Role::Tool
        );
    }
}
