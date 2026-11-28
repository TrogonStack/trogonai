//! The turns a session keeps once they are over.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::chat::{Message, ProviderReplay, ToolCall};
use crate::execution::{ExecutionMemory, UserTurn};
use crate::id::ToolName;
use crate::lifecycle::TurnPresentationOutcome;
use crate::result::CancelledCommandPresentation;
use crate::turn::TurnSummary;

/// A turn the model answered in full.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantHistoryTurn {
    /// What the user asked.
    pub user: UserTurn,
    /// What the model said.
    pub assistant: String,
    /// What it did on the way.
    pub execution: ExecutionMemory,
    /// The provider's own parts for the answer, for the routes that replay them.
    ///
    /// The steps inside [`Self::execution`] carry their own; this is the round
    /// that produced the answer and closed the turn, which is no step.
    pub provider_replay: Option<ProviderReplay>,
}

impl AssistantHistoryTurn {
    /// A turn that answered without touching anything.
    pub fn new(user: UserTurn, assistant: impl Into<String>) -> Self {
        Self {
            user,
            assistant: assistant.into(),
            execution: ExecutionMemory::default(),
            provider_replay: None,
        }
    }

    /// The same turn, carrying the provider's own parts for the answer.
    #[must_use]
    pub fn replaying(mut self, replay: ProviderReplay) -> Self {
        self.provider_replay = Some(replay);
        self
    }
}

/// How an interrupted turn ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptedTerminalReason {
    /// The user stopped it.
    #[default]
    Cancelled,
    /// It broke.
    Failed,
}

/// A turn that stopped before the model was done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptedHistoryTurn {
    /// What the user asked.
    pub user: UserTurn,
    /// What the model had said by then.
    pub assistant: Option<String>,
    /// The call that was in flight when it stopped.
    pub tool_call: Option<ToolCall>,
    /// The calls that had already finished.
    pub completed_tool_names: Vec<ToolName>,
    /// What it did before stopping.
    pub execution: ExecutionMemory,
    /// What was left of a command the stop killed.
    pub cancelled_command: Option<CancelledCommandPresentation>,
    /// Why it stopped.
    pub terminal_reason: InterruptedTerminalReason,
    /// What stopped it.
    pub cancellation_origin: CancellationOrigin,
}

impl InterruptedHistoryTurn {
    /// A turn the user stopped, with nothing in flight.
    #[must_use]
    pub fn cancelled(user: UserTurn) -> Self {
        Self {
            user,
            assistant: None,
            tool_call: None,
            completed_tool_names: Vec::new(),
            execution: ExecutionMemory::default(),
            cancelled_command: None,
            terminal_reason: InterruptedTerminalReason::Cancelled,
            cancellation_origin: CancellationOrigin::Turn,
        }
    }

    /// The same turn, recording that compaction cut it short rather than the user.
    #[must_use]
    pub const fn compacted(mut self) -> Self {
        self.cancellation_origin = CancellationOrigin::Compaction;
        self
    }
}

/// Text kept only so older session files still round-trip.
///
/// Model context and nothing more. None of it is permission authority, and it
/// sits behind a name that says so rather than beside the summary where it
/// could be mistaken for a record of what the user allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyCompactedContext {
    /// Root user text carried through earlier compactions.
    pub root_user_messages: Vec<String>,
    /// Whether that text is all of it.
    pub root_user_messages_complete: bool,
    /// Feedback carried through earlier compactions.
    pub permission_feedback: Vec<String>,
    /// Whether that feedback is all of it.
    pub permission_feedback_complete: bool,
}

impl Default for LegacyCompactedContext {
    fn default() -> Self {
        Self {
            root_user_messages: Vec::new(),
            root_user_messages_complete: true,
            permission_feedback: Vec::new(),
            permission_feedback_complete: true,
        }
    }
}

/// The turns that were replaced by a summary of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactedSummaryHistoryTurn {
    /// What the removed turns amounted to.
    pub summary: String,
    /// How many turns it replaced.
    pub removed_turn_count: usize,
    /// How many times this session has been compacted.
    pub compaction_count: usize,
    /// Text older session files carry.
    pub legacy: LegacyCompactedContext,
}

impl CompactedSummaryHistoryTurn {
    /// A summary of turns removed from the history.
    pub fn new(summary: impl Into<String>, removed_turn_count: usize, compaction_count: usize) -> Self {
        Self {
            summary: summary.into(),
            removed_turn_count,
            compaction_count,
            legacy: LegacyCompactedContext::default(),
        }
    }
}

/// What stopped a turn that did not finish.
///
/// A turn the user stopped and a turn compaction cut short both end with work
/// in flight, and only this says which: one is a decision the user made, the
/// other is the runtime making room, and a transcript that conflates them tells the
/// reader the user interrupted something they never touched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationOrigin {
    /// The turn itself was stopped.
    #[default]
    Turn,
    /// Compaction stopped it to reclaim context.
    Compaction,
}

/// Opens the span of a compaction's handoff text.
pub const CONTEXT_HANDOFF_OPEN: &str = "<context_handoff>";

/// Closes the span of a compaction's handoff text.
pub const CONTEXT_HANDOFF_CLOSE: &str = "</context_handoff>";

/// How much of the active context a compaction cut away.
///
/// A cut, not a second history: the turns behind it are still in the session
/// file, and this only says how much of them the model is no longer shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextHistoryCut {
    /// Turns dropped from the active context.
    pub turns: usize,
    /// Rounds of calls dropped from the turn at the boundary.
    pub tool_steps: usize,
    /// Pieces of steering dropped from that turn.
    pub steering: usize,
    /// Exact model-facing external-observation prefix dropped from that turn.
    #[serde(default, skip_serializing_if = "is_zero_count")]
    pub external_observations: usize,
}

fn is_zero_count(count: &usize) -> bool {
    *count == 0
}

/// One entry in a session's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryTurn {
    /// Turns replaced by a summary.
    CompactedSummary(CompactedSummaryHistoryTurn),
    /// A turn the model answered.
    Assistant(Box<AssistantHistoryTurn>),
    /// A turn that stopped early.
    Interrupted(Box<InterruptedHistoryTurn>),
}

impl HistoryTurn {
    /// What the user asked, for the turns a user started.
    #[must_use]
    pub fn user(&self) -> Option<&UserTurn> {
        match self {
            Self::CompactedSummary(_) => None,
            Self::Assistant(turn) => Some(&turn.user),
            Self::Interrupted(turn) => Some(&turn.user),
        }
    }

    /// What the user asked, for changing what is stored beside it.
    pub fn user_mut(&mut self) -> Option<&mut UserTurn> {
        match self {
            Self::CompactedSummary(_) => None,
            Self::Assistant(turn) => Some(&mut turn.user),
            Self::Interrupted(turn) => Some(&mut turn.user),
        }
    }

    /// What the model said, as far as it got.
    #[must_use]
    pub fn assistant_text(&self) -> Option<&str> {
        match self {
            Self::CompactedSummary(_) => None,
            Self::Assistant(turn) => Some(&turn.assistant),
            Self::Interrupted(turn) => turn.assistant.as_deref(),
        }
    }

    /// What the turn did to the workspace.
    #[must_use]
    pub fn execution(&self) -> Option<&ExecutionMemory> {
        match self {
            Self::CompactedSummary(_) => None,
            Self::Assistant(turn) => Some(&turn.execution),
            Self::Interrupted(turn) => Some(&turn.execution),
        }
    }

    /// What the turn did to the workspace, for recording what it cost.
    pub fn execution_mut(&mut self) -> Option<&mut ExecutionMemory> {
        match self {
            Self::CompactedSummary(_) => None,
            Self::Assistant(turn) => Some(&mut turn.execution),
            Self::Interrupted(turn) => Some(&mut turn.execution),
        }
    }

    /// What the turn cost, once it was measured.
    ///
    /// A compaction summary has no cost of its own: it stands in for turns that
    /// were each measured before they were replaced.
    #[must_use]
    pub fn summary(&self) -> Option<TurnSummary> {
        self.execution()?.turn_summary
    }

    /// Records what the turn cost, where the turn can hold it.
    pub fn set_summary(&mut self, summary: TurnSummary) {
        if let Some(execution) = self.execution_mut() {
            execution.turn_summary = Some(summary);
        }
    }
}

/// The highest compaction count recorded anywhere in a session history.
#[must_use]
pub fn latest_compaction_count(history: &[HistoryTurn]) -> usize {
    history
        .iter()
        .filter_map(|turn| match turn {
            HistoryTurn::CompactedSummary(summary) => Some(summary.compaction_count),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// Messages and history kept when compaction trims what a provider sees.
#[derive(Debug, Clone, PartialEq)]
pub struct RetainedCompactionWindow {
    /// Semantic source text the compactor reads.
    pub source: Vec<Message>,
    /// History turns still visible after the cut.
    pub retained_history: Vec<HistoryTurn>,
    /// Chat messages derived from the retained history.
    pub retained_messages: Vec<Message>,
    /// Where the retained window starts in the full history.
    pub cut: ContextHistoryCut,
    /// Tokens in the newest exchange the plan protected.
    pub newest_exchange_tokens: usize,
    /// Tokens estimated for the retained window.
    pub retained_tokens: usize,
}

impl From<AssistantHistoryTurn> for HistoryTurn {
    fn from(turn: AssistantHistoryTurn) -> Self {
        Self::Assistant(Box::new(turn))
    }
}

impl From<InterruptedHistoryTurn> for HistoryTurn {
    fn from(turn: InterruptedHistoryTurn) -> Self {
        Self::Interrupted(Box::new(turn))
    }
}

impl From<CompactedSummaryHistoryTurn> for HistoryTurn {
    fn from(turn: CompactedSummaryHistoryTurn) -> Self {
        Self::CompactedSummary(turn)
    }
}

/// How an interrupted turn is closed when history is replayed as chat messages.
///
/// A steering continuation omits the trailing `<turn_aborted>` closure because the current
/// prompt already carries steering intent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptedChatProjection {
    /// The default closure is appended after the interrupted turn.
    #[default]
    Closed,
    /// The handoff omits the abort notice because steering already said to continue.
    SteeringContinuation,
}

/// How many raw turns and projected messages a retained tail would cost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedHistoryTail {
    /// Raw turns kept at the end of the history.
    pub turn_count: usize,
    /// Chat messages those turns would project to.
    pub message_count: usize,
}

/// Which text a finished turn shows once it is no longer the live one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishedPromptProjection {
    /// Whatever the turn's own kind shows.
    #[default]
    HistoryDefault,
    /// The assistant text alone.
    AssistantText,
}

/// A hold on the snapshot files a finished turn still depends on.
///
/// An `Arc` is that refcount, so cloning the hold retains and dropping it releases. Only the
/// third operation survives as a method: handing the files over to whoever accepted the turn is
/// not something a refcount can express.
pub trait SnapshotFileHold: fmt::Debug + Send + Sync {
    /// Hands the files to whatever accepted the turn.
    fn transfer(&self);
}

/// A turn that has ended, on its way into the history.
#[derive(Debug, Clone)]
pub struct FinishedPrompt {
    /// The queued execution that produced this event, when its surface tracks one.
    /// Presentation lifecycle only; never part of the persisted history turn.
    pub queued_turn_id: Option<crate::TurnId>,
    /// The turn itself.
    pub turn: HistoryTurn,
    /// The text to show for it, when that differs from the text it keeps.
    ///
    /// Display only. The turn's own text is what the next request replays, so
    /// this never reaches a provider.
    pub presentation_text: Option<String>,
    /// What it cost, when it was measured.
    pub summary: Option<TurnSummary>,
    /// Which text it shows once it is history.
    pub terminal_projection: FinishedPromptProjection,
    /// How it ended, when the presentation cared.
    pub terminal_outcome: Option<TurnPresentationOutcome>,
    /// The snapshot files it holds until history takes them.
    pub snapshot_files: Option<Arc<dyn SnapshotFileHold>>,
}

impl FinishedPrompt {
    /// A finished turn holding nothing and measuring nothing.
    #[must_use]
    pub fn new(turn: HistoryTurn) -> Self {
        Self {
            queued_turn_id: None,
            turn,
            presentation_text: None,
            summary: None,
            terminal_projection: FinishedPromptProjection::HistoryDefault,
            terminal_outcome: None,
            snapshot_files: None,
        }
    }

    /// The same turn, showing this text in place of the text it keeps.
    #[must_use]
    pub fn showing(mut self, presentation_text: impl Into<String>) -> Self {
        self.presentation_text = Some(presentation_text.into());
        self
    }

    /// The same turn, holding snapshot files until history takes them.
    #[must_use]
    pub fn holding(mut self, snapshot_files: Arc<dyn SnapshotFileHold>) -> Self {
        self.snapshot_files = Some(snapshot_files);
        self
    }

    /// Hands the snapshot files over, if this turn holds any.
    pub fn accept_snapshot_files(&self) {
        if let Some(hold) = &self.snapshot_files {
            hold.transfer();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Debug, Default)]
    struct CountingHold {
        transfers: AtomicUsize,
    }

    impl SnapshotFileHold for CountingHold {
        fn transfer(&self) {
            self.transfers.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_compaction_has_no_user_behind_it_and_says_so() {
        let compacted: HistoryTurn = CompactedSummaryHistoryTurn::new("they refactored", 12, 1).into();

        assert!(compacted.user().is_none());
        assert!(compacted.assistant_text().is_none());
        assert!(compacted.execution().is_none());
    }

    #[test]
    fn an_interrupted_turn_keeps_the_text_the_model_got_out_before_stopping() {
        let turn: HistoryTurn = InterruptedHistoryTurn {
            assistant: Some("starting on it".to_owned()),
            ..InterruptedHistoryTurn::cancelled(UserTurn::new("refactor this"))
        }
        .into();

        assert_eq!(turn.assistant_text(), Some("starting on it"));
        assert_eq!(turn.user().map(|user| user.text.as_str()), Some("refactor this"));
    }

    #[test]
    fn legacy_compacted_context_defaults_to_complete_so_an_old_file_is_not_read_as_truncated() {
        let legacy = LegacyCompactedContext::default();

        assert!(legacy.root_user_messages_complete);
        assert!(legacy.permission_feedback_complete);
        assert!(legacy.permission_feedback.is_empty());
    }

    #[test]
    fn interrupted_chat_projection_defaults_to_closed() {
        assert_eq!(InterruptedChatProjection::default(), InterruptedChatProjection::Closed);
    }

    #[test]
    fn retained_history_tail_starts_empty() {
        assert_eq!(RetainedHistoryTail::default().turn_count, 0);
        assert_eq!(RetainedHistoryTail::default().message_count, 0);
    }

    #[test]
    fn history_taking_a_turn_hands_over_the_snapshot_files_it_was_holding() {
        let hold = Arc::new(CountingHold::default());
        let finished =
            FinishedPrompt::new(AssistantHistoryTurn::new(UserTurn::new("check the build"), "it builds").into())
                .holding(hold.clone());

        finished.accept_snapshot_files();
        assert_eq!(hold.transfers.load(Ordering::Relaxed), 1);

        FinishedPrompt::new(CompactedSummaryHistoryTurn::new("summary", 1, 1).into()).accept_snapshot_files();
        assert_eq!(
            hold.transfers.load(Ordering::Relaxed),
            1,
            "a turn holding nothing hands nothing over"
        );
    }

    #[test]
    fn latest_compaction_count_reads_the_highest_summary_marker() {
        let history: Vec<HistoryTurn> = vec![
            CompactedSummaryHistoryTurn::new("first", 3, 1).into(),
            CompactedSummaryHistoryTurn::new("second", 2, 4).into(),
        ];
        assert_eq!(latest_compaction_count(&history), 4);
        assert_eq!(latest_compaction_count(&[]), 0);
    }
}
