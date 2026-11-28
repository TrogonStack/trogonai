//! What a tool call is doing, reported while it is still doing it.

use serde::{Deserialize, Serialize};

use crate::{ToolCallId, ToolName, ToolResultMemory, TurnTokens, UnixMillis};

/// Identity of one turn within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(u64);

impl TurnId {
    /// Names a turn by its number.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Identity of one step within a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StepId(u64);

impl StepId {
    /// Names a step by its number.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// How a turn ended, as far as anyone watching is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPresentationOutcome {
    /// The model said what it had to say.
    Completed,
    /// Somebody stopped it.
    Interrupted,
    /// It stopped on its own, badly.
    Failed,
    /// It is waiting for something before it can go on.
    Paused,
}

/// The last thing said about a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnFinished {
    /// The turn.
    pub turn: TurnId,
    /// How it ended.
    pub outcome: TurnPresentationOutcome,
}

/// Identity of one tool call, as the transcript refers to it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolLifecycleId {
    /// The turn the call belongs to.
    pub turn: TurnId,
    /// The call, as the provider named it.
    pub call: ToolCallId,
}

impl ToolLifecycleId {
    /// Names a call within a turn.
    #[must_use]
    pub fn new(turn: TurnId, call: ToolCallId) -> Self {
        Self { turn, call }
    }
}

/// Identity of the block several tool calls are drawn inside.
///
/// Tool calls that arrive together are shown as one group anchored on the step
/// that opened it, so a surface can keep them from scattering as each finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolPresentationGroupId {
    /// The turn the group belongs to.
    pub turn: TurnId,
    /// The step the group is drawn against.
    pub anchor_step: StepId,
}

impl ToolPresentationGroupId {
    /// Names a group by the step it hangs from.
    #[must_use]
    pub const fn new(turn: TurnId, anchor_step: StepId) -> Self {
        Self { turn, anchor_step }
    }
}

/// What a tool call turned out to be doing, in the vocabulary a surface draws.
///
/// This is coarser than the tool's own name on purpose: a surface has an icon
/// and a verb for "reading something", and gets a new tool every few weeks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolActivityKind {
    /// Reading a file.
    Read,
    /// Listing or searching.
    List,
    /// Writing a file that was not there.
    Write,
    /// Changing a file that was.
    Edit,
    /// Opening something outside trogonai.
    Open,
    /// Running a command.
    Command,
    /// Handing work to a subagent.
    Subagent,
    /// Asking the person something.
    Ask,
    /// Calling a tool published by an MCP server.
    Mcp,
}

impl ToolActivityKind {
    /// All supported values in stable enumeration order.
    pub const ALL: [Self; 9] = [
        Self::Read,
        Self::List,
        Self::Write,
        Self::Edit,
        Self::Open,
        Self::Command,
        Self::Subagent,
        Self::Ask,
        Self::Mcp,
    ];

    /// Where this kind's tally sits in [`ActivityCounts`].
    const fn slot(self) -> usize {
        match self {
            Self::Read => 0,
            Self::List => 1,
            Self::Write => 2,
            Self::Edit => 3,
            Self::Open => 4,
            Self::Command => 5,
            Self::Subagent => 6,
            Self::Ask => 7,
            Self::Mcp => 8,
        }
    }
}

/// How much of each kind of work a turn has done.
///
/// Counters are keyed by the activity kind so reporting work cannot forget
/// its tally. Legacy slots keep their positions and new kinds append slots,
/// allowing older saved stream snapshots to decode without shifting counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(transparent)]
pub struct ActivityCounts([u32; ToolActivityKind::ALL.len()]);

impl<'de> Deserialize<'de> for ActivityCounts {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Counters;
        impl<'de> serde::de::Visitor<'de> for Counters {
            type Value = ActivityCounts;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("eight or nine activity counters")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
                let mut counts = [0; ToolActivityKind::ALL.len()];
                let mut length = 0;
                while let Some(count) = sequence.next_element::<u32>()? {
                    if length == counts.len() {
                        return Err(serde::de::Error::invalid_length(length + 1, &self));
                    }
                    counts[length] = count;
                    length += 1;
                }
                if !matches!(length, 8 | 9) {
                    return Err(serde::de::Error::invalid_length(length, &self));
                }
                Ok(ActivityCounts(counts))
            }
        }
        deserializer.deserialize_seq(Counters)
    }
}

impl ActivityCounts {
    /// Counts one more of a kind.
    pub fn record(&mut self, kind: ToolActivityKind) {
        let slot = &mut self.0[kind.slot()];
        *slot = slot.saturating_add(1);
    }

    /// How many of a kind have been counted.
    #[must_use]
    pub const fn of(&self, kind: ToolActivityKind) -> u32 {
        self.0[kind.slot()]
    }

    /// How many of everything.
    #[must_use]
    pub fn total(&self) -> u32 {
        self.0.iter().copied().fold(0, u32::saturating_add)
    }
}

/// When the current turn started and whether it is waiting on somebody.
///
/// The elapsed counter a surface shows is the difference between now and
/// [`Self::started`], minus whatever the person spent answering a prompt. The
/// reference does that subtraction by moving `turn_started_ms` forward on
/// resume, which is why the two fields are recorded together here: they are one
/// clock, and the second one adjusts the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TurnTiming {
    /// When the turn started, or `None` while no turn is running.
    ///
    /// Moves forward when a wait ends, so the elapsed count excludes the wait.
    pub started: Option<UnixMillis>,
    /// When the runtime started waiting on the person, or `None` if it is not waiting.
    pub waiting_since: Option<UnixMillis>,
}

impl TurnTiming {
    /// Whether the clock shown to the person is frozen.
    #[must_use]
    pub const fn is_waiting(&self) -> bool {
        self.waiting_since.is_some()
    }
}

/// What kind of work a turn is doing, for the one word the status row names it with.
///
/// Replaces the pair of booleans this state used to carry. Two independent flags
/// could describe a turn that had started printing and was also composing a tool
/// payload and was also delegating, which is three answers to a question that has
/// exactly one; the phase is that one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    /// Nothing has printed yet.
    #[default]
    Thinking,
    /// Assistant text is reaching the transcript.
    Generating,
    /// A tool is doing the work, so nothing will print for a while.
    Running,
    /// A subagent holds the turn.
    WaitingForSubagent,
}

impl TurnPhase {
    /// The marked word the status row names this phase with.
    ///
    /// [`Self::WaitingForSubagent`] shares `Running`'s word: the person is
    /// waiting on a tool either way, and saying which kind of tool would be
    /// naming an implementation detail of the turn.
    #[must_use]
    pub const fn marked_label(self) -> &'static str {
        match self {
            Self::Thinking => "\u{2022} Thinking",
            Self::Generating => "\u{2022} Generating",
            Self::Running | Self::WaitingForSubagent => "\u{2022} Running",
        }
    }
}

/// A turn reporting which phase it moved into.
///
/// The step carries the ordering: an update from a step older than the one the
/// state already settled on is stale and does not apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnPhaseUpdate {
    /// The turn that changed phase.
    pub turn_id: TurnId,
    /// The step the change belongs to.
    pub step_id: StepId,
    /// The phase the turn moved into.
    pub phase: TurnPhase,
}

/// What a turn is doing right now, for the row that says so.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StreamState {
    /// Whether a turn is under way.
    pub active: bool,
    /// How many chunks have arrived.
    pub chunks: usize,
    /// How much of each kind of work the turn has done.
    pub activity: ActivityCounts,
    /// Tokens spent so far.
    pub token_progress: TurnTokens,
    /// The last kind of work started, which is what the row names.
    pub last_activity_kind: Option<ToolActivityKind>,
    /// The turn's clock.
    pub timing: TurnTiming,
    /// What kind of work the turn is doing.
    pub phase: TurnPhase,
    /// The step [`Self::phase`] was settled by, or `None` while no step has.
    pub phase_step_id: Option<StepId>,
}

impl StreamState {
    /// Adopts `update`'s phase, answering whether the phase actually moved.
    ///
    /// An update is ignored unless a turn is under way, `update` names the turn
    /// `active_turn` says is running, and its step is not older than the one
    /// that settled the current phase. The active turn arrives as an argument
    /// because this crate holds no worker: keeping the whole rule in one place
    /// is worth one parameter, since a caller that checked only part of it would
    /// let a finished turn repaint the row.
    pub fn apply_phase(&mut self, active_turn: Option<TurnId>, update: TurnPhaseUpdate) -> bool {
        if !self.active || active_turn != Some(update.turn_id) {
            return false;
        }
        if self.phase_step_id.is_some_and(|settled| update.step_id < settled) {
            return false;
        }
        let moved = self.phase != update.phase;
        self.phase = update.phase;
        self.phase_step_id = Some(update.step_id);
        moved
    }
}

/// How a tool call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcomeKind {
    /// It ran and finished.
    Completed,
    /// A permission decision stopped it.
    Denied,
    /// Somebody stopped it.
    Cancelled,
    /// It ran and failed.
    Failed,
    /// It was never run, and the model was told so.
    Deferred,
}

/// How a tool call ended, and the one line saying so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    /// How it ended.
    pub kind: ToolOutcomeKind,
    /// What to say about it.
    pub summary: String,
}

impl ToolOutcome {
    /// Records an ending and its summary.
    #[must_use]
    pub fn new(kind: ToolOutcomeKind, summary: impl Into<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
        }
    }
}

/// A tool call the model has started naming, which the runtime has not accepted yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionalTool {
    /// The call.
    pub id: ToolLifecycleId,
    /// The block it is drawn in, when it belongs to one.
    pub presentation_group: Option<ToolPresentationGroupId>,
    /// The tool, once enough of its name has streamed to know it.
    pub tool_name: Option<ToolName>,
    /// What it looks like it is going to do.
    pub activity_kind: ToolActivityKind,
}

/// A tool call the runtime has accepted and is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartedTool {
    /// The call.
    pub id: ToolLifecycleId,
    /// The block it is drawn in, when it belongs to one.
    pub presentation_group: Option<ToolPresentationGroupId>,
    /// The provisional entry this one settles, when there was one.
    pub reconciles: Option<ToolCallId>,
    /// The tool.
    pub tool_name: ToolName,
    /// What it is doing.
    pub activity_kind: ToolActivityKind,
    /// The arguments, as the provider serialized them.
    pub arguments_json: Option<String>,
    /// Whether it belongs below everything already in the transcript.
    ///
    /// A tool that starts after the model has moved on reads as part of the
    /// previous block unless it is placed deliberately.
    pub place_after_current_transcript: bool,
    /// What the row names the call after, when the runtime resolved it.
    #[serde(skip)]
    pub resolved_display_target: Option<crate::ResolvedDisplayTarget>,
}

/// A tool call that has stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishedTool {
    /// The call.
    pub id: ToolLifecycleId,
    /// How it ended.
    pub outcome: ToolOutcome,
    /// What it produced, when that is worth showing.
    pub result: Option<String>,
    /// What the transcript stops saying about the result.
    pub result_memory: Option<ToolResultMemory>,
    /// The handle to a command's captured output, when there is one.
    pub command_artifact_handle: Option<String>,
}

/// One thing that happened to a tool call, or to the turn holding it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolLifecycleEvent {
    /// The model is naming a tool.
    Provisional(ProvisionalTool),
    /// The runtime accepted a tool and started it.
    AuthoritativeStarted(StartedTool),
    /// A running tool said something about how it is going.
    Progress {
        /// The call.
        id: ToolLifecycleId,
        /// What it said.
        text: String,
    },
    /// A tool stopped.
    Terminal(FinishedTool),
    /// The turn stopped.
    TurnFinished(TurnFinished),
}

impl ToolLifecycleEvent {
    /// The call this event is about, when it is about one.
    #[must_use]
    pub fn tool(&self) -> Option<&ToolLifecycleId> {
        match self {
            Self::Provisional(tool) => Some(&tool.id),
            Self::AuthoritativeStarted(tool) => Some(&tool.id),
            Self::Progress { id, .. } => Some(id),
            Self::Terminal(tool) => Some(&tool.id),
            Self::TurnFinished(_) => None,
        }
    }

    /// The turn this event belongs to.
    #[must_use]
    pub fn turn(&self) -> TurnId {
        match self {
            Self::Provisional(tool) => tool.id.turn,
            Self::AuthoritativeStarted(tool) => tool.id.turn,
            Self::Progress { id, .. } => id.turn,
            Self::Terminal(tool) => tool.id.turn,
            Self::TurnFinished(finished) => finished.turn,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(turn: u64, id: &str) -> ToolLifecycleId {
        ToolLifecycleId::new(TurnId::new(turn), ToolCallId::new(id).expect("a call id"))
    }

    #[test]
    fn legacy_activity_counts_keep_their_slots_without_an_mcp_tally() {
        let counts: ActivityCounts = serde_json::from_str("[1,2,3,4,5,6,7,8]").expect("legacy counters");
        assert_eq!(counts.of(ToolActivityKind::Command), 6);
        assert_eq!(counts.of(ToolActivityKind::Ask), 8);
        assert_eq!(counts.of(ToolActivityKind::Mcp), 0);
        let mut current = counts;
        current.record(ToolActivityKind::Mcp);
        let encoded = serde_json::to_string(&current).expect("current counters");
        assert_eq!(
            serde_json::from_str::<ActivityCounts>(&encoded).expect("roundtrip"),
            current
        );
        assert!(serde_json::from_str::<ActivityCounts>("[1,2]").is_err());
    }

    #[test]
    fn every_event_names_the_turn_it_belongs_to() {
        let id = call(7, "r-call");
        let events = [
            ToolLifecycleEvent::Provisional(ProvisionalTool {
                id: id.clone(),
                presentation_group: None,
                tool_name: None,
                activity_kind: ToolActivityKind::Read,
            }),
            ToolLifecycleEvent::Progress {
                id: id.clone(),
                text: "half way".to_owned(),
            },
            ToolLifecycleEvent::Terminal(FinishedTool {
                id: id.clone(),
                outcome: ToolOutcome::new(ToolOutcomeKind::Completed, "read 40 lines"),
                result: None,
                result_memory: None,
                command_artifact_handle: None,
            }),
            ToolLifecycleEvent::TurnFinished(TurnFinished {
                turn: TurnId::new(7),
                outcome: TurnPresentationOutcome::Completed,
            }),
        ];

        for event in &events {
            assert_eq!(event.turn(), TurnId::new(7));
        }
        assert_eq!(events[0].tool(), Some(&id));
        assert_eq!(events[3].tool(), None);
    }

    #[test]
    fn an_authoritative_start_says_which_provisional_entry_it_settles() {
        let started = StartedTool {
            id: call(1, "call_authoritative"),
            presentation_group: Some(ToolPresentationGroupId::new(TurnId::new(1), StepId::new(4))),
            reconciles: Some(ToolCallId::new("call_provisional").expect("a call id")),
            tool_name: ToolName::new("read_file").expect("a tool name"),
            activity_kind: ToolActivityKind::Read,
            arguments_json: Some("{\"path\":\"README.md\"}".to_owned()),
            place_after_current_transcript: false,
            resolved_display_target: None,
        };

        assert_eq!(
            started.reconciles.as_ref().map(ToolCallId::as_str),
            Some("call_provisional"),
        );
        assert_ne!(started.id.call.as_str(), "call_provisional");
    }

    #[test]
    fn a_turn_counts_each_kind_of_work_separately() {
        let mut counts = ActivityCounts::default();
        counts.record(ToolActivityKind::Read);
        counts.record(ToolActivityKind::Read);
        counts.record(ToolActivityKind::Command);

        assert_eq!(counts.of(ToolActivityKind::Read), 2);
        assert_eq!(counts.of(ToolActivityKind::Command), 1);
        assert_eq!(counts.of(ToolActivityKind::Edit), 0);
        assert_eq!(counts.total(), 3);
    }

    #[test]
    fn every_activity_kind_has_a_tally_of_its_own() {
        let mut counts = ActivityCounts::default();
        for kind in ToolActivityKind::ALL {
            counts.record(kind);
        }
        for kind in ToolActivityKind::ALL {
            assert_eq!(counts.of(kind), 1);
        }
        assert_eq!(counts.total(), ToolActivityKind::ALL.len() as u32);
    }

    #[test]
    fn a_turn_that_has_not_started_has_no_clock_to_show() {
        let state = StreamState::default();
        assert!(!state.active);
        assert_eq!(state.timing.started, None);
        assert!(!state.timing.is_waiting());

        let waiting = TurnTiming {
            started: Some(UnixMillis::new(1_000)),
            waiting_since: Some(UnixMillis::new(2_500)),
        };
        assert!(waiting.is_waiting());
    }

    #[test]
    fn a_lifecycle_event_survives_a_round_trip_through_json() {
        let event = ToolLifecycleEvent::Terminal(FinishedTool {
            id: call(3, "call_9"),
            outcome: ToolOutcome::new(ToolOutcomeKind::Denied, "not allowed to write there"),
            result: None,
            result_memory: None,
            command_artifact_handle: Some("artifact-1".to_owned()),
        });

        let encoded = serde_json::to_string(&event).expect("an event encodes");
        assert!(encoded.contains("\"terminal\""));
        assert_eq!(
            serde_json::from_str::<ToolLifecycleEvent>(&encoded).expect("an event decodes"),
            event,
        );
    }
}
