//! What a finished tool call leaves in the session record.

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::chat::ToolStatus;
use crate::hash::ContentHash;
use crate::id::{ToolCallId, ToolName};
use crate::lifecycle::{ToolLifecycleId, ToolOutcomeKind};
use crate::memory::ToolResultMemory;
use crate::time::UnixMillis;

/// A durable tool outcome, preserving explicit permission denial provenance.
///
/// Legacy failures carry no denial provenance and remain ordinary errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedToolStatus {
    /// The tool ran and produced a result.
    Success,
    /// The tool did not, without a more specific recorded cause.
    Failure,
    /// Permission was denied before the tool could execute.
    Denied,
}

impl PersistedToolStatus {
    /// Whether the call succeeded.
    #[must_use]
    pub const fn succeeded(self) -> bool {
        matches!(self, Self::Success)
    }

    /// The name this outcome is written as.
    ///
    /// Deliberately the same spelling the serde representation uses, so a
    /// status a person reads in a session report and one a reader parses out
    /// of the same session are the same word.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denied => "denied",
        }
    }
}

impl From<ToolStatus> for PersistedToolStatus {
    fn from(status: ToolStatus) -> Self {
        match status {
            ToolStatus::Success => Self::Success,
            ToolStatus::Error | ToolStatus::Cancelled => Self::Failure,
            ToolStatus::Denied => Self::Denied,
        }
    }
}

/// Why a call was recorded without having run.
///
/// Naming the classification means a caller can act on which deferral it was instead of
/// matching the sentence back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDeferral {
    /// The call was abandoned before execution.
    NotExecuted,
    /// Project instructions arrived first, so the call has to be reissued.
    ContextChanged,
}

impl ToolDeferral {
    /// The exact output text a record carries for this deferral.
    #[must_use]
    pub const fn output(self) -> &'static str {
        match self {
            Self::NotExecuted => "Not executed",
            Self::ContextChanged => concat!(
                "Scoped project instructions were added before execution. ",
                "Review them and reissue this tool call if it is still appropriate."
            ),
        }
    }

    /// The status word shown in place of a failure, where there is one.
    #[must_use]
    pub const fn status_label(self) -> Option<&'static str> {
        match self {
            Self::NotExecuted => None,
            Self::ContextChanged => Some("Reading project instructions before continuing:"),
        }
    }

    /// Recognises an output the runtime wrote itself.
    ///
    /// The match is exact: a tool that happens to print `not executed`, or the
    /// same sentence with a trailing newline, is reporting its own failure and
    /// is not a deferral.
    #[must_use]
    pub fn of_output(output: &str) -> Option<Self> {
        [Self::NotExecuted, Self::ContextChanged]
            .into_iter()
            .find(|deferral| deferral.output() == output)
    }
}

/// Whether the file existed before the call that committed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommittedFileKind {
    /// The call created the file.
    Added,
    /// The call changed a file that was already there.
    Edited,
}

/// What one line of a committed change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommittedFileLineKind {
    /// Unchanged, shown for orientation.
    Context,
    /// Added by the call.
    Addition,
    /// Removed by the call.
    Deletion,
    /// A run of lines left out.
    Elision,
    /// A remark about the change rather than part of it.
    Notice,
}

/// One line of a committed change.
///
/// Which line numbers exist follows from the kind: an addition has no line in
/// the old file, a deletion has none in the new one, and an elision stands for
/// lines rather than being one. The constructors are the only way to say it, so
/// a deletion cannot be built claiming a new line number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedFileLine {
    kind: CommittedFileLineKind,
    old_line: Option<u32>,
    new_line: Option<u32>,
    text: String,
}

impl CommittedFileLine {
    /// A line both files hold.
    pub fn context(old_line: u32, new_line: u32, text: impl Into<String>) -> Self {
        Self {
            kind: CommittedFileLineKind::Context,
            old_line: Some(old_line),
            new_line: Some(new_line),
            text: text.into(),
        }
    }

    /// A line only the new file holds.
    pub fn addition(new_line: u32, text: impl Into<String>) -> Self {
        Self {
            kind: CommittedFileLineKind::Addition,
            old_line: None,
            new_line: Some(new_line),
            text: text.into(),
        }
    }

    /// A line only the old file held.
    pub fn deletion(old_line: u32, text: impl Into<String>) -> Self {
        Self {
            kind: CommittedFileLineKind::Deletion,
            old_line: Some(old_line),
            new_line: None,
            text: text.into(),
        }
    }

    /// A stand-in for lines not shown.
    pub fn elision(text: impl Into<String>) -> Self {
        Self {
            kind: CommittedFileLineKind::Elision,
            old_line: None,
            new_line: None,
            text: text.into(),
        }
    }

    /// A remark about the change.
    pub fn notice(text: impl Into<String>) -> Self {
        Self {
            kind: CommittedFileLineKind::Notice,
            old_line: None,
            new_line: None,
            text: text.into(),
        }
    }

    /// A line as a session file recorded it.
    ///
    /// The other constructors decide the line numbers from the kind, which is
    /// what keeps a deletion from being built claiming a line in the new file.
    /// A file written by an earlier runtime is read as it stands rather than as this
    /// one would have written it, so reading is the one place that says all
    /// four parts.
    pub fn restored(
        kind: CommittedFileLineKind,
        old_line: Option<u32>,
        new_line: Option<u32>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            old_line,
            new_line,
            text: text.into(),
        }
    }

    /// The same line, with its text said differently.
    ///
    /// Redaction rewrites what a line says without changing which line it is,
    /// so the kind and the two line numbers travel unchanged and only the text
    /// is replaced.
    #[must_use]
    pub fn saying(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// What this line is.
    #[must_use]
    pub const fn kind(&self) -> CommittedFileLineKind {
        self.kind
    }

    /// Where the line sat in the old file.
    #[must_use]
    pub const fn old_line(&self) -> Option<u32> {
        self.old_line
    }

    /// Where the line sits in the new file.
    #[must_use]
    pub const fn new_line(&self) -> Option<u32> {
        self.new_line
    }

    /// The line itself.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// How many lines a change added and removed.
///
/// Counted over the whole change, which is not the same as counting the lines
/// kept: a truncated presentation still reports the real totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DiffStat {
    /// Lines the change added.
    pub additions: usize,
    /// Lines the change removed.
    pub deletions: usize,
}

/// One whole-file snapshot a committed change carries, either kept beside the
/// change or spilled to a session artifact.
///
/// A durable record/checkpoint grows with every large edit a session makes if
/// whole-file snapshots always travel inline; this is the typed, exclusive
/// authority that keeps that cost bounded while the full bytes stay
/// recoverable. A snapshot is either [`Self::Inline`], or it has been
/// [`Self::Spilled`] to an artifact named by `handle`, whose length and digest
/// this value also carries so a reader can tell a missing or corrupt artifact
/// from the one that was actually written.
///
/// Small snapshots serialize exactly as a plain string, so a session written
/// before this type existed, or one whose snapshots never crossed the spill
/// threshold, reads back unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommittedFileContent {
    /// The snapshot, kept beside the change.
    Inline(String),
    /// The snapshot, spilled to the artifact named by `handle`.
    Spilled {
        /// The artifact's name, carrying its own family prefix and digest.
        handle: String,
        /// The snapshot's length in bytes, independent of the artifact.
        length: usize,
        /// The digest the artifact is expected to match.
        digest: ContentHash,
        /// A bounded prefix of the snapshot, kept beside the change so a
        /// reader has something to show when the artifact is unavailable.
        preview: String,
    },
}

impl CommittedFileContent {
    /// A snapshot kept beside the change.
    #[must_use]
    pub fn inline(text: impl Into<String>) -> Self {
        Self::Inline(text.into())
    }

    /// A snapshot spilled to an artifact.
    #[must_use]
    pub fn spilled(handle: impl Into<String>, length: usize, digest: ContentHash, preview: impl Into<String>) -> Self {
        Self::Spilled {
            handle: handle.into(),
            length,
            digest,
            preview: preview.into(),
        }
    }

    /// The snapshot's length in bytes, inline or spilled.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Inline(text) => text.len(),
            Self::Spilled { length, .. } => *length,
        }
    }

    /// Whether the snapshot is of zero length.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The snapshot's bytes, when it was kept beside the change rather than
    /// spilled.
    #[must_use]
    pub fn as_inline(&self) -> Option<&str> {
        match self {
            Self::Inline(text) => Some(text),
            Self::Spilled { .. } => None,
        }
    }

    /// The artifact this snapshot was spilled to, when it was spilled.
    #[must_use]
    pub fn handle(&self) -> Option<&str> {
        match self {
            Self::Inline(_) => None,
            Self::Spilled { handle, .. } => Some(handle),
        }
    }

    /// The digest the spilled artifact is expected to match, when spilled.
    #[must_use]
    pub const fn digest(&self) -> Option<&ContentHash> {
        match self {
            Self::Inline(_) => None,
            Self::Spilled { digest, .. } => Some(digest),
        }
    }

    /// A bounded preview of the snapshot: the whole text when inline, the
    /// carried preview when spilled.
    #[must_use]
    pub fn preview(&self) -> &str {
        match self {
            Self::Inline(text) => text,
            Self::Spilled { preview, .. } => preview,
        }
    }
}

impl Serialize for CommittedFileContent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Inline(text) => serializer.serialize_str(text),
            Self::Spilled {
                handle,
                length,
                digest,
                preview,
            } => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("handle", handle)?;
                map.serialize_entry("length", length)?;
                map.serialize_entry("digest", &digest.to_string())?;
                map.serialize_entry("preview", preview)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for CommittedFileContent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ContentVisitor;

        impl<'de> Visitor<'de> for ContentVisitor {
            type Value = CommittedFileContent;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a snapshot string or a spilled-artifact map")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(CommittedFileContent::Inline(value.to_owned()))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(CommittedFileContent::Inline(value))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut handle: Option<String> = None;
                let mut length: Option<usize> = None;
                let mut digest: Option<String> = None;
                let mut preview: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "handle" => handle = Some(map.next_value()?),
                        "length" => length = Some(map.next_value()?),
                        "digest" => digest = Some(map.next_value()?),
                        "preview" => preview = Some(map.next_value()?),
                        _ => {
                            let _ignored: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let handle = handle.ok_or_else(|| de::Error::missing_field("handle"))?;
                let length = length.ok_or_else(|| de::Error::missing_field("length"))?;
                let digest = digest.ok_or_else(|| de::Error::missing_field("digest"))?;
                let preview = preview.ok_or_else(|| de::Error::missing_field("preview"))?;
                let digest =
                    ContentHash::parse(&digest).ok_or_else(|| de::Error::custom("digest is not a content hash"))?;
                Ok(CommittedFileContent::Spilled {
                    handle,
                    length,
                    digest,
                    preview,
                })
            }
        }

        deserializer.deserialize_any(ContentVisitor)
    }
}

/// Captured review rows, inline or retained beside the session log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommittedFileReview {
    /// Rows small enough to travel in the log frame.
    Inline(Vec<CommittedFileLine>),
    /// Rows whose serialized bytes are retained in a verified artifact.
    Artifact(CommittedFileReviewArtifact),
}

/// An artifact containing serialized committed review rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommittedFileReviewArtifact {
    /// The session-local artifact name.
    pub handle: String,
    /// The exact serialized byte count.
    pub length: usize,
    /// The expected content digest.
    pub digest: ContentHash,
}

impl CommittedFileReview {
    /// Rows available without opening an artifact.
    #[must_use]
    pub fn as_inline(&self) -> Option<&[CommittedFileLine]> {
        match self {
            Self::Inline(rows) => Some(rows),
            Self::Artifact(_) => None,
        }
    }
}

/// A file change a tool call committed, as a later reader can see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedFilePresentation {
    /// The file the call wrote.
    pub path: String,
    /// Whether the file existed beforehand.
    pub kind: CommittedFileKind,
    /// The change, line by line, possibly abridged.
    pub lines: Vec<CommittedFileLine>,
    /// Totals over the whole change.
    #[serde(flatten)]
    pub stat: DiffStat,
    /// Whether `lines` leaves part of the change out.
    pub truncated: bool,
    /// What the file held before, when it was captured.
    pub previous_content: Option<CommittedFileContent>,
    /// What the file holds now, when it was captured.
    pub after_content: Option<CommittedFileContent>,
    /// The call this change came from.
    pub lifecycle_id: Option<ToolLifecycleId>,
    /// Captured full-review rows, used instead of complete file snapshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_lines: Option<CommittedFileReview>,
}

impl CommittedFilePresentation {
    /// Whether captured rows have one lifecycle and no competing file snapshots.
    #[must_use]
    pub fn full_view_source_is_valid(&self) -> bool {
        self.review_lines.is_none()
            || (self.lifecycle_id.is_some() && self.previous_content.is_none() && self.after_content.is_none())
    }

    /// A change with nothing elided and no content captured.
    pub fn new(
        path: impl Into<String>,
        kind: CommittedFileKind,
        lines: Vec<CommittedFileLine>,
        stat: DiffStat,
    ) -> Self {
        Self {
            path: path.into(),
            kind,
            lines,
            stat,
            truncated: false,
            previous_content: None,
            after_content: None,
            lifecycle_id: None,
            review_lines: None,
        }
    }

    /// The same change, recording that `lines` leaves part of it out.
    #[must_use]
    pub fn abridged(mut self) -> Self {
        self.truncated = true;
        self
    }

    /// The same change, carrying the call it came from.
    #[must_use]
    pub fn from_call(mut self, id: ToolLifecycleId) -> Self {
        self.lifecycle_id = Some(id);
        self
    }
}

/// Where the framed output of a command was put.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutputReplayDescriptor {
    /// What the store was given to retrieve the output by.
    pub handle: String,
    /// Size of the framed output behind that handle.
    pub framed_bytes: usize,
}

/// Whether a command's output can still be replayed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutputReplay {
    /// The output is held and can be shown again.
    Available(CommandOutputReplayDescriptor),
    /// The output is gone.
    Unavailable,
}

impl CommandOutputReplay {
    /// The descriptor, when the output is still held.
    #[must_use]
    pub const fn descriptor(&self) -> Option<&CommandOutputReplayDescriptor> {
        match self {
            Self::Available(descriptor) => Some(descriptor),
            Self::Unavailable => None,
        }
    }
}

/// What was left of a command that the user stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelledCommandPresentation {
    /// Whether the partial output survived the cancellation.
    pub output_replay: Option<CommandOutputReplay>,
    /// The artifact the run wrote, if it got that far.
    pub command_artifact_handle: Option<String>,
}

/// How a command's process ended.
///
/// It is the process's own outcome, and a signal is not an exit code: a command killed by
/// `SIGKILL` and one that returned `9` are different events that a single integer conflates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandProcessOutcome {
    /// The process returned this status.
    Exited {
        /// The status it returned.
        code: i64,
    },
    /// The process was killed by this signal.
    Signaled {
        /// The signal that killed it.
        signal: u32,
    },
    /// The command outlived the time it was given, so it has no status of its own.
    TimedOut,
    /// The command ran, and the runtime lost the output rather than the process.
    OutputCaptureFailed,
}

/// How a terminal action that reached the terminal ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalReturnPresentation {
    /// The session was started and left running.
    Started,
    /// What the action was waiting for happened.
    ConditionMet,
    /// The action stopped itself before it could run away.
    SafetyCeiling,
    /// Somebody stopped it.
    Cancelled,
    /// The program in the terminal returned this status.
    Exited {
        /// The status it returned.
        code: i32,
    },
    /// The program in the terminal was killed by this signal.
    Signaled {
        /// The signal that killed it.
        signal: u32,
    },
}

/// Why a terminal action never reached the terminal.
///
/// Distinct from [`TerminalReturnPresentation::Exited`] with a non-zero status:
/// the program never ran, so there is nothing to report about what it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalFailurePresentation {
    /// The request could not be read as a terminal action.
    InvalidRequest,
    /// The path named lies outside the workspace.
    PathOutsideWorkspace,
    /// This host offers no terminal.
    UnsupportedHost,
    /// The host offers a terminal but no shell to run in it.
    ShellUnavailable,
    /// The host could not give the session a pseudo-terminal.
    PtyUnavailable,
    /// The session was created and did not come up.
    StartupFailed,
    /// The host could not say which process the session is.
    ProcessIdentityUnavailable,
    /// The session existed and is gone.
    SessionLost,
    /// The session named was never here.
    SessionNotFound,
    /// The session is not in a state this action is allowed from.
    InvalidLifecycle,
    /// The authority the action presented was refused.
    AuthorityDenied,
    /// The authority was written by a runtime old enough that it no longer counts.
    AuthorityRetired,
    /// Somebody else holds the control lease.
    LeaseConflict,
    /// Output between the last cursor and this one was lost.
    CursorGap,
    /// The session's screen could not be read.
    ScreenUnavailable,
    /// The session speaks a terminal protocol this runtime does not.
    ProtocolIncompatible,
    /// The host is already running as many terminals as it allows.
    CapacityExceeded,
    /// Somebody stopped the action before it was attempted.
    Cancelled,
}

impl TerminalFailurePresentation {
    /// The one line to show for this failure.
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid request",
            Self::PathOutsideWorkspace => "path is outside the workspace",
            Self::UnsupportedHost => "terminal host is unavailable",
            Self::ShellUnavailable => "terminal shell is unavailable",
            Self::PtyUnavailable => "terminal PTY is unavailable",
            Self::StartupFailed => "terminal startup failed",
            Self::ProcessIdentityUnavailable => "terminal process identity is unavailable",
            Self::SessionLost => "terminal session was lost",
            Self::SessionNotFound => "terminal session not found",
            Self::InvalidLifecycle => "terminal session is in an invalid lifecycle state",
            Self::AuthorityDenied => "terminal authority denied",
            Self::AuthorityRetired => {
                "saved terminal authority is from an older trogonai version; start a new terminal"
            }
            Self::LeaseConflict => "terminal control lease conflict",
            Self::CursorGap => "terminal output cursor gap",
            Self::ScreenUnavailable => "terminal screen is unavailable",
            Self::ProtocolIncompatible => "terminal protocol is incompatible",
            Self::CapacityExceeded => "terminal capacity exceeded",
            Self::Cancelled => "terminal action was cancelled",
        }
    }
}

/// How one terminal action ended, as a later reader can see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalActionPresentation {
    /// The action reached the terminal.
    Returned(TerminalReturnPresentation),
    /// It did not.
    Failed(TerminalFailurePresentation),
}

impl TerminalActionPresentation {
    /// How the call that ran this action ended.
    ///
    /// A cancellation is not a failure on either side of the split: the action
    /// was stopped, and saying it failed would put a problem where a decision
    /// was.
    #[must_use]
    pub const fn outcome_kind(self) -> ToolOutcomeKind {
        match self {
            Self::Returned(returned) => match returned {
                TerminalReturnPresentation::Started
                | TerminalReturnPresentation::ConditionMet
                | TerminalReturnPresentation::SafetyCeiling => ToolOutcomeKind::Completed,
                TerminalReturnPresentation::Cancelled => ToolOutcomeKind::Cancelled,
                TerminalReturnPresentation::Exited { code: 0 } => ToolOutcomeKind::Completed,
                TerminalReturnPresentation::Exited { .. } | TerminalReturnPresentation::Signaled { .. } => {
                    ToolOutcomeKind::Failed
                }
            },
            Self::Failed(TerminalFailurePresentation::Cancelled) => ToolOutcomeKind::Cancelled,
            Self::Failed(_) => ToolOutcomeKind::Failed,
        }
    }
}

/// What a call left somewhere other than the transcript.
///
/// Grouped rather than sitting loose on the memory because only two kinds of
/// call produce any of them, and a struct nobody fills should not cost every
/// result that carries one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultArtifacts {
    /// The file change the call committed.
    pub committed_file: Option<CommittedFilePresentation>,
    /// Whether a command's output survives to be replayed.
    pub command_output_replay: Option<CommandOutputReplay>,
    /// How a command's process ended.
    pub command_process: Option<CommandProcessOutcome>,
    /// How a terminal action ended.
    pub terminal_action: Option<TerminalActionPresentation>,
}

/// One finished tool call, as the session file remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedToolResult {
    /// The call this answers.
    pub call: ToolCallId,
    /// The tool that was asked to run.
    pub tool_name: ToolName,
    /// Whether it worked.
    pub status: PersistedToolStatus,
    /// The text the model was shown.
    pub output: String,
    /// The facts about that text the text no longer states.
    pub memory: ToolResultMemory,
    /// Whether the provider ran the tool rather than trogonai.
    pub provider_native: bool,
    /// Host review feedback retained for the agent, not security evidence.
    pub review_feedback: bool,
    /// When the result was recorded.
    ///
    /// Absence is absence here.
    pub created_at: Option<UnixMillis>,
    /// What the user said when asked to allow the call.
    pub permission_feedback: Vec<String>,
}

impl PersistedToolResult {
    /// A result whose output was kept whole in the transcript.
    pub fn new(call: ToolCallId, tool_name: ToolName, status: PersistedToolStatus, output: impl Into<String>) -> Self {
        let output = output.into();
        let bytes = output.len();
        Self {
            call,
            tool_name,
            status,
            output,
            memory: ToolResultMemory::inline(bytes, bytes, false),
            provider_native: false,
            review_feedback: false,
            created_at: None,
            permission_feedback: Vec::new(),
        }
    }

    /// The same result, marked as host review feedback.
    #[must_use]
    pub const fn with_review_feedback(mut self, review_feedback: bool) -> Self {
        self.review_feedback = review_feedback;
        self
    }

    /// A record for a call that never ran.
    pub fn deferred(call: ToolCallId, tool_name: ToolName, deferral: ToolDeferral) -> Self {
        Self::new(call, tool_name, PersistedToolStatus::Failure, deferral.output())
    }

    /// The same result, with what is remembered about its output.
    #[must_use]
    pub fn remembering(mut self, memory: ToolResultMemory) -> Self {
        self.memory = memory;
        self
    }

    /// The same result, recorded at an instant.
    #[must_use]
    pub const fn recorded_at(mut self, created_at: UnixMillis) -> Self {
        self.created_at = Some(created_at);
        self
    }

    /// Why the call did not run, when it did not.
    #[must_use]
    pub fn deferral(&self) -> Option<ToolDeferral> {
        match self.status {
            PersistedToolStatus::Success | PersistedToolStatus::Denied => None,
            PersistedToolStatus::Failure => ToolDeferral::of_output(&self.output),
        }
    }

    /// Whether the call was recorded without having run.
    #[must_use]
    pub fn is_deferred(&self) -> bool {
        self.deferral().is_some()
    }

    /// Whether the call was held back by new project instructions.
    #[must_use]
    pub fn is_context_deferred(&self) -> bool {
        self.deferral() == Some(ToolDeferral::ContextChanged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(status: PersistedToolStatus, output: &str) -> PersistedToolResult {
        PersistedToolResult::new(
            ToolCallId::new("call_deferred").expect("a call id"),
            ToolName::new("read_file").expect("a tool name"),
            status,
            output,
        )
    }

    #[test]
    fn a_status_a_person_reads_is_the_same_word_a_reader_parses() {
        for status in [PersistedToolStatus::Success, PersistedToolStatus::Failure] {
            assert_eq!(
                serde_json::to_value(status).expect("a status is a name"),
                serde_json::Value::from(status.label())
            );
        }
    }

    #[test]
    fn a_deferred_result_is_recognised_only_by_the_exact_sentence_trogonai_wrote() {
        let deferred = result(PersistedToolStatus::Failure, ToolDeferral::NotExecuted.output());
        assert!(deferred.is_deferred());
        assert!(!deferred.is_context_deferred());

        for near_miss in ["Not executed\n", "not executed", "tool failed"] {
            let other = result(PersistedToolStatus::Failure, near_miss);
            assert!(!other.is_deferred(), "{near_miss:?} is a tool's own failure");
        }
    }

    #[test]
    fn a_succeeding_call_is_never_a_deferral_whatever_it_printed() {
        let succeeded = result(PersistedToolStatus::Success, ToolDeferral::NotExecuted.output());
        assert!(!succeeded.is_deferred());
        assert!(!succeeded.is_context_deferred());
    }

    #[test]
    fn a_context_deferral_is_both_a_deferral_and_the_context_one() {
        let deferred = result(PersistedToolStatus::Failure, ToolDeferral::ContextChanged.output());
        assert!(deferred.is_deferred());
        assert!(deferred.is_context_deferred());
        assert_eq!(
            deferred.deferral().and_then(ToolDeferral::status_label),
            Some("Reading project instructions before continuing:")
        );
    }

    #[test]
    fn deferring_a_call_writes_the_output_that_classifies_it() {
        let deferred = PersistedToolResult::deferred(
            ToolCallId::new("r-call").expect("a call id"),
            ToolName::new("write_file").expect("a tool name"),
            ToolDeferral::ContextChanged,
        );
        assert_eq!(deferred.deferral(), Some(ToolDeferral::ContextChanged));
        assert_eq!(deferred.memory.output_bytes(), deferred.output.len());
    }

    #[test]
    fn a_cancelled_call_is_recorded_as_a_failure_the_file_can_hold() {
        assert_eq!(
            PersistedToolStatus::from(ToolStatus::Cancelled),
            PersistedToolStatus::Failure
        );
        assert!(PersistedToolStatus::from(ToolStatus::Success).succeeded());
    }

    #[test]
    fn a_deletion_carries_no_line_number_in_the_file_it_is_missing_from() {
        let deletion = CommittedFileLine::deletion(12, "old text");
        assert_eq!(deletion.old_line(), Some(12));
        assert_eq!(deletion.new_line(), None);

        let addition = CommittedFileLine::addition(12, "new text");
        assert_eq!(addition.old_line(), None);
        assert_eq!(addition.new_line(), Some(12));

        let elision = CommittedFileLine::elision("40 lines");
        assert_eq!(elision.old_line(), None);
        assert_eq!(elision.new_line(), None);
    }

    #[test]
    fn an_abridged_change_still_reports_the_totals_of_the_whole_change() {
        let presentation = CommittedFilePresentation::new(
            "src/main.rs",
            CommittedFileKind::Edited,
            vec![CommittedFileLine::elision("120 lines")],
            DiffStat {
                additions: 80,
                deletions: 40,
            },
        )
        .abridged();

        assert_eq!(presentation.lines.len(), 1);
        assert_eq!(presentation.stat.additions, 80);
        assert!(presentation.truncated);
    }

    #[test]
    fn a_lost_command_output_offers_no_handle_to_fetch_it_by() {
        assert_eq!(CommandOutputReplay::Unavailable.descriptor(), None);
        let available = CommandOutputReplay::Available(CommandOutputReplayDescriptor {
            handle: "run-7".to_owned(),
            framed_bytes: 4096,
        });
        assert_eq!(available.descriptor().map(|held| held.handle.as_str()), Some("run-7"));
    }

    #[test]
    fn a_terminal_action_somebody_stopped_is_cancelled_from_either_side() {
        assert_eq!(
            TerminalActionPresentation::Returned(TerminalReturnPresentation::Cancelled).outcome_kind(),
            ToolOutcomeKind::Cancelled
        );
        assert_eq!(
            TerminalActionPresentation::Failed(TerminalFailurePresentation::Cancelled).outcome_kind(),
            ToolOutcomeKind::Cancelled
        );
    }

    #[test]
    fn only_a_zero_status_from_the_terminal_completed_the_call() {
        assert_eq!(
            TerminalActionPresentation::Returned(TerminalReturnPresentation::Exited { code: 0 }).outcome_kind(),
            ToolOutcomeKind::Completed
        );
        assert_eq!(
            TerminalActionPresentation::Returned(TerminalReturnPresentation::Exited { code: 1 }).outcome_kind(),
            ToolOutcomeKind::Failed
        );
        assert_eq!(
            TerminalActionPresentation::Returned(TerminalReturnPresentation::Signaled { signal: 9 }).outcome_kind(),
            ToolOutcomeKind::Failed
        );
        assert_eq!(
            TerminalActionPresentation::Returned(TerminalReturnPresentation::SafetyCeiling).outcome_kind(),
            ToolOutcomeKind::Completed
        );
    }

    #[test]
    fn a_terminal_that_never_started_is_not_a_terminal_that_returned_a_status() {
        let never_started = TerminalActionPresentation::Failed(TerminalFailurePresentation::ShellUnavailable);

        assert_eq!(never_started.outcome_kind(), ToolOutcomeKind::Failed);
        assert_eq!(
            TerminalFailurePresentation::ShellUnavailable.detail(),
            "terminal shell is unavailable"
        );
    }
}
