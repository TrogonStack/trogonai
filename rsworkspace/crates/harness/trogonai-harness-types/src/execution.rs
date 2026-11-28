//! What a turn actually did, kept for the turns that follow it.

use serde::{Deserialize, Serialize};

use crate::chat::{ImageAttachment, ProviderReplay, ToolCall};
use crate::id::{MessageId, ToolCallId, ToolName, WorkId};
use crate::result::{PersistedToolResult, PersistedToolStatus};
use crate::turn::TurnSummary;

/// What a tool call did to a file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileEvidenceAction {
    /// The call read the file.
    Read,
    /// The call wrote the file whole.
    Write,
    /// The call changed part of the file.
    Edit,
    /// The call removed the file.
    Delete,
    /// The call moved the file to `new_path`.
    Rename,
    /// The call duplicated the file to `new_path`.
    Copy,
    /// The call searched the file's contents.
    Search,
    /// The call listed the file rather than opening it.
    List,
    /// The call touched the file in a way the record does not name.
    #[default]
    Unknown,
}

impl FileEvidenceAction {
    /// The name this action is written as.
    ///
    /// Deliberately the same spelling the serde representation uses, so an
    /// action a person reads in a session report and one a reader parses out
    /// of the same session are the same word.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Edit => "edit",
            Self::Delete => "delete",
            Self::Rename => "rename",
            Self::Copy => "copy",
            Self::Search => "search",
            Self::List => "list",
            Self::Unknown => "unknown",
        }
    }

    /// Whether this action leaves the file with a second path.
    #[must_use]
    pub const fn has_destination(self) -> bool {
        matches!(self, Self::Rename | Self::Copy)
    }
}

/// One file a tool call touched, and what it did to it.
///
/// The destination of a rename stays a separate field rather than riding on the
/// action, because this record is written to the session file in exactly that
/// shape and read back by versions of the runtime that expect it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEvidence {
    /// The file the call named.
    pub path: String,
    /// Where a rename or a copy put it.
    pub new_path: Option<String>,
    /// The call that touched it.
    pub call: ToolCallId,
    /// The tool that ran.
    pub tool_name: ToolName,
    /// What it did.
    pub action: FileEvidenceAction,
    /// Whether it worked.
    pub status: PersistedToolStatus,
    /// Whether the model was shown the whole file rather than part of it.
    pub model_view_covers_full_file: bool,
    /// Whether the file has changed since, so this no longer describes it.
    pub stale: bool,
}

impl FileEvidence {
    /// A successful action on one file, with the model's view of it unproven.
    pub fn new(path: impl Into<String>, call: ToolCallId, tool_name: ToolName, action: FileEvidenceAction) -> Self {
        Self {
            path: path.into(),
            new_path: None,
            call,
            tool_name,
            action,
            status: PersistedToolStatus::Success,
            model_view_covers_full_file: false,
            stale: false,
        }
    }

    /// The same evidence, naming where the file ended up.
    #[must_use]
    pub fn moved_to(mut self, new_path: impl Into<String>) -> Self {
        self.new_path = Some(new_path.into());
        self
    }

    /// The same evidence, recording that the call failed.
    #[must_use]
    pub const fn failed(mut self) -> Self {
        self.status = PersistedToolStatus::Failure;
        self
    }

    /// The same evidence, recording that the model saw the whole file.
    #[must_use]
    pub const fn showing_whole_file(mut self) -> Self {
        self.model_view_covers_full_file = true;
        self
    }

    /// Where the file is now.
    #[must_use]
    pub fn resulting_path(&self) -> &str {
        self.new_path.as_deref().unwrap_or(&self.path)
    }
}

/// One round of the turn: what the model said, what it asked for, what it got.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecutionStep {
    /// What the model said before asking for the calls.
    pub assistant: Option<String>,
    /// The calls it asked for.
    pub tool_calls: Vec<ToolCall>,
    /// What they answered.
    pub tool_results: Vec<PersistedToolResult>,
    /// The provider's own parts for this round, for the routes that replay them.
    pub provider_replay: Option<ProviderReplay>,
}

impl ToolExecutionStep {
    /// A round with no assistant text before it.
    #[must_use]
    pub fn new(tool_calls: Vec<ToolCall>, tool_results: Vec<PersistedToolResult>) -> Self {
        Self {
            assistant: None,
            tool_calls,
            tool_results,
            provider_replay: None,
        }
    }

    /// The same round, carrying the provider's own parts for this step.
    #[must_use]
    pub fn replaying(mut self, replay: ProviderReplay) -> Self {
        self.provider_replay = Some(replay);
        self
    }

    /// The same round, with the text the model said before asking.
    #[must_use]
    pub fn said(mut self, assistant: impl Into<String>) -> Self {
        self.assistant = Some(assistant.into());
        self
    }

    /// The result of one of this round's calls.
    #[must_use]
    pub fn result_for(&self, call: &ToolCallId) -> Option<&PersistedToolResult> {
        self.tool_results.iter().find(|result| &result.call == call)
    }
}

/// Whether a steering message reached the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteeringDelivery {
    /// Waiting to be applied.
    Queued,
    /// Applied by the child.
    Applied,
    /// Refused or could not be applied.
    NotApplied,
}

/// Something the user said while the turn was already running.
///
/// Kept with the round it arrived after, because guidance the user gave before
/// the third round of calls and guidance they gave after it are different
/// instructions, and a replay that loses the boundary reorders them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSteering {
    /// What the user said.
    pub text: String,
    /// What the model had said when the guidance arrived.
    pub assistant_prefix: Option<String>,
    /// How many rounds of calls had finished by then.
    pub after_tool_steps: usize,
}

impl PersistedSteering {
    /// Guidance that arrived after this many rounds of calls.
    pub fn new(text: impl Into<String>, after_tool_steps: usize) -> Self {
        Self {
            text: text.into(),
            assistant_prefix: None,
            after_tool_steps,
        }
    }

    /// The same guidance, with what the model had said when it arrived.
    #[must_use]
    pub fn interrupting(mut self, assistant_prefix: impl Into<String>) -> Self {
        self.assistant_prefix = Some(assistant_prefix.into());
        self
    }
}

/// Everything a turn did, in the order it did it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionMemory {
    /// External data anchored to the execution boundary where it was applied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_observations: Vec<crate::external_observation::PersistedExternalObservation>,
    /// Trusted context updates in execution order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_notes: Vec<crate::context_note::PersistedContextNote>,
    /// The initial runtime context retained across session reconstruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_runtime_context: Option<crate::context_note::CapturedRuntimeContext>,
    /// The rounds of calls the turn made.
    pub tool_steps: Vec<ToolExecutionStep>,
    /// The files those calls touched.
    pub files: Vec<FileEvidence>,
    /// Guidance the user gave between rounds, with the round it followed.
    pub steering: Vec<PersistedSteering>,
    /// What the turn cost, once it finished.
    pub turn_summary: Option<TurnSummary>,
}

impl ExecutionMemory {
    /// Whether the turn did nothing worth remembering.
    ///
    /// Guidance counts. A turn that ran no calls and touched no files but was
    /// steered mid-flight still has something a later turn needs to read.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tool_steps.is_empty()
            && self.files.is_empty()
            && self.steering.is_empty()
            && self.context_notes.is_empty()
            && self.external_observations.is_empty()
            && self.initial_runtime_context.is_none()
    }

    /// The most recent evidence about one path, under either of its names.
    #[must_use]
    pub fn evidence_for(&self, path: &str) -> Option<&FileEvidence> {
        self.files
            .iter()
            .rev()
            .find(|file| file.path == path || file.new_path.as_deref() == Some(path))
    }
}

/// What the user submitted to start a turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserTurn {
    /// What they typed.
    pub text: String,
    /// What they attached.
    pub images: Vec<ImageAttachment>,
    /// The child work this turn belongs to, when a manager owns it.
    pub work_id: Option<WorkId>,
    /// The id a client was told this message goes by, when one was. Absent
    /// on a turn no ACP version 2 client sent, and on every turn stored
    /// before ids were kept, which a replay then names afresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<MessageId>,
}

impl UserTurn {
    /// A turn of plain text.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
            work_id: None,
            message_id: None,
        }
    }

    /// The same turn, with what the user attached to it.
    #[must_use]
    pub fn with_images(mut self, images: Vec<ImageAttachment>) -> Self {
        self.images = images;
        self
    }

    /// The same turn, joined to the child work a manager owns.
    #[must_use]
    pub fn for_work(mut self, work_id: WorkId) -> Self {
        self.work_id = Some(work_id);
        self
    }

    /// The same turn, named the way a client was told it is.
    #[must_use]
    pub fn named(mut self, message_id: MessageId) -> Self {
        self.message_id = Some(message_id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str) -> ToolCallId {
        ToolCallId::new(id).expect("a call id")
    }

    fn tool(name: &str) -> ToolName {
        ToolName::new(name).expect("a tool name")
    }

    #[test]
    fn an_action_a_person_reads_is_the_same_word_a_reader_parses() {
        for action in [
            FileEvidenceAction::Read,
            FileEvidenceAction::Write,
            FileEvidenceAction::Edit,
            FileEvidenceAction::Delete,
            FileEvidenceAction::Rename,
            FileEvidenceAction::Copy,
            FileEvidenceAction::Search,
            FileEvidenceAction::List,
            FileEvidenceAction::Unknown,
        ] {
            assert_eq!(
                serde_json::to_value(action).expect("an action is a name"),
                serde_json::Value::from(action.label())
            );
        }
    }

    #[test]
    fn a_renamed_file_is_found_under_the_name_it_now_has() {
        let memory = ExecutionMemory {
            tool_steps: Vec::new(),
            files: vec![
                FileEvidence::new("src/old.rs", call("c1"), tool("rename"), FileEvidenceAction::Rename)
                    .moved_to("src/new.rs"),
            ],
            ..ExecutionMemory::default()
        };

        assert_eq!(
            memory.evidence_for("src/new.rs").map(FileEvidence::resulting_path),
            Some("src/new.rs")
        );
        assert!(memory.evidence_for("src/old.rs").is_some());
        assert!(memory.evidence_for("src/other.rs").is_none());
    }

    #[test]
    fn a_turn_that_touched_nothing_is_empty_even_after_the_model_spoke() {
        assert!(ExecutionMemory::default().is_empty());

        let spoke = ExecutionMemory {
            tool_steps: vec![ToolExecutionStep::default().said("thinking about it")],
            ..ExecutionMemory::default()
        };
        assert!(
            !spoke.is_empty(),
            "a recorded step is history whether or not it called anything"
        );

        let steered = ExecutionMemory {
            steering: vec![PersistedSteering::new("use the other file", 0)],
            ..ExecutionMemory::default()
        };
        assert!(
            !steered.is_empty(),
            "guidance the user gave mid-turn is history even if nothing ran"
        );
    }

    #[test]
    fn evidence_that_named_no_destination_still_answers_where_the_file_is() {
        let read = FileEvidence::new("src/main.rs", call("c2"), tool("read"), FileEvidenceAction::Read);

        assert_eq!(read.resulting_path(), "src/main.rs");
        assert!(!read.action.has_destination());
        assert!(FileEvidenceAction::Copy.has_destination());
    }

    #[test]
    fn a_step_answers_for_the_call_that_asked_rather_than_the_one_beside_it() {
        let step = ToolExecutionStep::new(
            Vec::new(),
            vec![
                PersistedToolResult::new(call("c1"), tool("read"), PersistedToolStatus::Success, "one"),
                PersistedToolResult::new(call("c2"), tool("read"), PersistedToolStatus::Failure, "two"),
            ],
        );

        assert_eq!(
            step.result_for(&call("c2")).map(|result| result.status),
            Some(PersistedToolStatus::Failure)
        );
        assert!(step.result_for(&call("c3")).is_none());
    }

    #[test]
    fn a_managed_turn_carries_its_join_key_without_changing_what_the_model_sees() {
        let turn = UserTurn::new("fix the build").for_work(WorkId::new("job-9").expect("a work id"));

        assert_eq!(turn.text, "fix the build");
        assert!(turn.images.is_empty());
        assert_eq!(turn.work_id.as_ref().map(WorkId::as_str), Some("job-9"));
    }
}
