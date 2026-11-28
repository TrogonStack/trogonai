//! What a provider said a turn was, and whether the runtime may act on it.
//!
//! Two questions are asked of every completion and they are deliberately
//! separate. Disposition asks what happened: did the turn finish, get truncated,
//! fail, or stop mid-flight. Admission asks whether the tool calls in it are
//! trustworthy enough to run. A completion can be well-formed and inadmissible,
//! and running its tools anyway is how a provider's bad payload becomes the runtime's
//! side effect.

use serde::{Deserialize, Serialize};

use crate::{Billing, FinishReason, Provenance, ProviderUsage, ToolCall};

/// Why a tool call's identity on the wire was unusable.
///
/// A constructed [`ToolCall`] always has a valid id, so this describes what a
/// gateway payload contained, not a state a call can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolIdentityFault {
    /// No id field.
    Absent,
    /// An id field holding an empty string.
    Empty,
    /// An id field that was not a string.
    WrongType,
}

impl ToolIdentityFault {
    /// The wire spelling, which is what a message about the fault names.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Empty => "empty",
            Self::WrongType => "wrong_type",
        }
    }
}

/// Why a provider-executed result could not be trusted.
///
/// Reported by whoever decodes the gateway payload. Admission refuses the whole
/// completion when one is present rather than dropping the offending call,
/// because a payload the runtime cannot fully account for is one where the missing part
/// may be the part that mattered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderResultFault {
    /// The call's identity was unusable.
    Identity(ToolIdentityFault),
    /// A result arrived for an id no call claimed.
    Unmatched,
    /// More than one call could own the result.
    Ambiguous,
    /// A provider-executed call arrived without its result.
    MissingResult,
    /// The stream ended mid-argument.
    IncompleteStreamedInput,
    /// Arguments were present but not usable.
    InvalidToolInput,
    /// Two payload fields disagreed on the tool's name.
    ConflictingToolName,
    /// A provider-executed entry was structurally malformed.
    MalformedProviderExecuted,
    /// The payload claimed both provenances for one call.
    ProvenanceContradiction,
    /// A preliminary entry was malformed.
    MalformedPreliminary,
    /// Two results claimed the same call.
    DuplicateResult,
}

impl ProviderResultFault {
    /// The wire spelling, which is what a message about the fault names.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Identity(fault) => fault.label(),
            Self::Unmatched => "unmatched",
            Self::Ambiguous => "ambiguous",
            Self::MissingResult => "missing_result",
            Self::IncompleteStreamedInput => "incomplete_streamed_input",
            Self::InvalidToolInput => "invalid_tool_input",
            Self::ConflictingToolName => "conflicting_tool_name",
            Self::MalformedProviderExecuted => "malformed_provider_executed",
            Self::ProvenanceContradiction => "provenance_contradiction",
            Self::MalformedPreliminary => "malformed_preliminary",
            Self::DuplicateResult => "duplicate_result",
        }
    }
}

/// Why a provider turn failed, when the failure says something about whether
/// asking again could work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureCause {
    /// The gateway stopped producing a stream it had opened.
    GatewayStreamTimeout,
    /// The provider refused in a way that asking again will not change.
    NonRetryable,
    /// The provider asked the runtime to slow down.
    RateLimited,
}

/// The settled payload of one provider turn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderCompletion {
    /// Assistant text, when the turn produced any.
    pub content: Option<String>,
    /// Tool calls the model requested, in the order it requested them.
    pub tool_calls: Vec<ToolCall>,
    /// Generation identity, when the gateway reported a well-formed one.
    pub generation_id: Option<crate::GenerationId>,
    /// The gateway provider that served the turn, when the gateway said.
    pub served_provider: Option<crate::ServedProvider>,
    /// The processing tier explicitly confirmed by the provider.
    pub served_service_tier: Option<crate::service_tier::ServedServiceTier>,
    /// Exact usage, when the gateway reported it.
    pub billing: Option<Billing>,
    /// What the provider said the turn spent, as far as it said anything.
    ///
    /// Separate from `billing`: a turn can report its token counts and still
    /// have no priced record, because a price needs a timestamp, a resolved
    /// model and a cost that the counts alone do not carry.
    pub usage: ProviderUsage,
    /// Generation or resolved-model metadata was malformed or self-contradicting.
    pub generation_metadata_invalid: bool,
    /// An earlier delivery may have billed outside this generation identity.
    pub delivery_ambiguous: bool,
    /// A fault the payload decoder found in the provider-executed results.
    pub provider_result_fault: Option<ProviderResultFault>,
    /// Why the provider turn failed, when the failure said.
    pub provider_failure_cause: Option<ProviderFailureCause>,
    /// Detail for a provider-reported failure, for the user rather than the model.
    pub provider_failure_detail: Option<String>,
    /// The provider's own response items, to hand back on the next request of
    /// this turn. Opaque to the runtime, and meaningful only to the route that wrote it.
    pub provider_state_json: Option<String>,
    /// Why the provider stopped. Absent means the stream ended without saying,
    /// which is an interruption rather than a completion.
    pub finish_reason: Option<FinishReason>,
}

/// What happened to a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// The turn ended the way it said it would.
    Completed,
    /// Output hit the token ceiling, so the answer is cut off.
    LengthLimited,
    /// The stream ended without a finish reason.
    Interrupted,
    /// The provider refused or errored.
    ProviderFailure,
    /// The finish reason and the payload contradict each other.
    InvalidCompletion,
}

/// The longest identifier a conversation record can hold.
pub const MAX_CONVERSATION_IDENTITY_BYTES: usize = 256;

/// Which identifier of a tool call a conversation record could not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityField {
    /// The id the call settled under.
    Id,
    /// The tool the call named.
    Name,
    /// The id the call was first announced under.
    ProvisionalId,
}

impl IdentityField {
    /// The wire spelling, which is what a message about the field names.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Name => "name",
            Self::ProvisionalId => "provisional_id",
        }
    }
}

/// Whether a completion's tool calls may be run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAdmission {
    /// Every call is accounted for and may be dispatched.
    Admitted,
    /// The payload decoder reported a fault in the provider-executed results.
    RejectedProviderResult(ProviderResultFault),
    /// A provider-executed call carried arguments that are not one complete
    /// JSON document.
    RejectedProviderArguments,
    /// Two calls in one completion claim the same id.
    RejectedDuplicateIdentity,
    /// A call named itself at a length no conversation record can hold, so
    /// running it would produce history that cannot be written back.
    RejectedUnstorableIdentity(IdentityField),
}

impl ProviderCompletion {
    /// Classifies what happened to the turn.
    ///
    /// A finish reason that promises tool calls without carrying any, or a
    /// terminal reason carrying local calls the runtime was never asked to run, is
    /// invalid rather than complete: the provider described a turn that is not
    /// the turn it sent.
    #[must_use]
    pub fn disposition(&self) -> Disposition {
        let Some(finish_reason) = &self.finish_reason else {
            return Disposition::Interrupted;
        };
        match finish_reason {
            FinishReason::ProviderError | FinishReason::ContentFilter => Disposition::ProviderFailure,
            FinishReason::Length => Disposition::LengthLimited,
            FinishReason::ToolCalls => {
                if self.tool_calls.is_empty() {
                    Disposition::InvalidCompletion
                } else {
                    Disposition::Completed
                }
            }
            FinishReason::Stop => {
                if self.tool_calls.is_empty() || self.all_tool_calls_provider_executed() {
                    Disposition::Completed
                } else {
                    Disposition::InvalidCompletion
                }
            }
            FinishReason::Other => {
                if self.tool_calls.is_empty() {
                    Disposition::Completed
                } else {
                    Disposition::InvalidCompletion
                }
            }
        }
    }

    /// Whether every call was already run by the provider, so a terminal finish
    /// reason alongside them is coherent.
    ///
    /// An empty list is not "all", because the question being asked is whether
    /// there is provider-side work that explains a `stop`.
    #[must_use]
    pub fn all_tool_calls_provider_executed(&self) -> bool {
        !self.tool_calls.is_empty()
            && self
                .tool_calls
                .iter()
                .all(|call| matches!(call.provenance, Provenance::ProviderExecuted { .. }))
    }

    /// Whether the provider has already both spoken and done everything it asked
    /// for, leaving the runtime nothing to run and nothing to wait for.
    ///
    /// The three conditions travel together on purpose: a `stop` with no words
    /// is not the provider ending the turn on an answer, and a `stop` alongside
    /// a call the runtime still owes is not the provider having finished at all.
    #[must_use]
    pub fn is_terminal_provider_executed(&self) -> bool {
        self.finish_reason == Some(FinishReason::Stop)
            && self.content.as_ref().is_some_and(|content| !content.is_empty())
            && self.all_tool_calls_provider_executed()
    }

    /// Decides whether the tool calls in this completion may be dispatched.
    ///
    /// Malformed arguments are fatal only for a provider-executed call, whose
    /// result is already computed and therefore already wrong. A local call with
    /// unparseable arguments is admitted on purpose: dispatch turns it into a
    /// tool error the model can read and correct, which is a better turn than a
    /// silently dropped call.
    #[must_use]
    pub fn tool_admission(&self) -> ToolAdmission {
        if let Some(fault) = self.provider_result_fault {
            return ToolAdmission::RejectedProviderResult(fault);
        }
        if self.has_duplicate_identity() {
            return ToolAdmission::RejectedDuplicateIdentity;
        }
        if let Some(field) = self.unstorable_identity() {
            return ToolAdmission::RejectedUnstorableIdentity(field);
        }
        if self.tool_calls.iter().any(|call| {
            matches!(call.provenance, Provenance::ProviderExecuted { .. }) && !call.arguments_are_wellformed()
        }) {
            return ToolAdmission::RejectedProviderArguments;
        }
        ToolAdmission::Admitted
    }

    /// Whether the agent loop should dispatch tools and take another turn.
    ///
    /// Three conditions, deliberately in one place: the turn has to have
    /// completed coherently, its calls have to be admissible, and at least one
    /// of them has to be the runtime's to run. Split across call sites, this is where a
    /// caller checks two of the three and dispatches a payload the third would
    /// have refused.
    #[must_use]
    pub fn wants_tool_dispatch(&self) -> bool {
        self.disposition() == Disposition::Completed
            && self.tool_admission() == ToolAdmission::Admitted
            && self
                .tool_calls
                .iter()
                .any(|call| matches!(call.provenance, Provenance::Local))
    }

    fn has_duplicate_identity(&self) -> bool {
        self.tool_calls
            .iter()
            .enumerate()
            .any(|(index, call)| self.tool_calls[..index].iter().any(|prior| prior.id == call.id))
    }

    /// The first identifier a conversation record could not hold, if any.
    fn unstorable_identity(&self) -> Option<IdentityField> {
        self.tool_calls.iter().find_map(|call| {
            if !storable(call.id.as_str()) {
                return Some(IdentityField::Id);
            }
            if !storable(call.name.as_str()) {
                return Some(IdentityField::Name);
            }
            match &call.provisional_id {
                Some(provisional) if !storable(provisional.as_str()) => Some(IdentityField::ProvisionalId),
                _ => None,
            }
        })
    }
}

fn storable(identity: &str) -> bool {
    identity.len() <= MAX_CONVERSATION_IDENTITY_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolCallId, ToolName};

    fn local(id: &str) -> ToolCall {
        ToolCall::local(
            ToolCallId::new(id).unwrap(),
            ToolName::new("read_file").unwrap(),
            "{\"path\":\"a\"}".to_string(),
        )
    }

    fn provider_executed(id: &str) -> ToolCall {
        ToolCall::provider_executed(
            ToolCallId::new(id).unwrap(),
            ToolName::new("web_search").unwrap(),
            "{}".to_string(),
            "{\"results\":[]}".to_string(),
        )
    }

    fn completion(finish_reason: Option<FinishReason>, tool_calls: Vec<ToolCall>) -> ProviderCompletion {
        ProviderCompletion {
            finish_reason,
            tool_calls,
            ..ProviderCompletion::default()
        }
    }

    #[test]
    fn an_identity_no_record_could_hold_is_refused_before_its_arguments_are_read() {
        let oversized = "n".repeat(MAX_CONVERSATION_IDENTITY_BYTES + 1);
        let cases = [
            (
                ToolCall::local(
                    ToolCallId::new(&oversized).expect("a non-empty id"),
                    ToolName::new("read_file").expect("a non-empty name"),
                    "{".to_owned(),
                ),
                IdentityField::Id,
            ),
            (
                ToolCall::local(
                    ToolCallId::new("r-call").expect("a non-empty id"),
                    ToolName::new(&oversized).expect("a non-empty name"),
                    "{".to_owned(),
                ),
                IdentityField::Name,
            ),
            (
                ToolCall::local(
                    ToolCallId::new("r-call").expect("a non-empty id"),
                    ToolName::new("read_file").expect("a non-empty name"),
                    "{".to_owned(),
                )
                .announced_as(ToolCallId::new(&oversized).expect("a non-empty id")),
                IdentityField::ProvisionalId,
            ),
        ];

        for (call, field) in cases {
            assert_eq!(
                completion(Some(FinishReason::ToolCalls), vec![call]).tool_admission(),
                ToolAdmission::RejectedUnstorableIdentity(field)
            );
        }

        let borderline = "n".repeat(MAX_CONVERSATION_IDENTITY_BYTES);
        assert_eq!(
            completion(
                Some(FinishReason::ToolCalls),
                vec![ToolCall::local(
                    ToolCallId::new(&borderline).expect("a non-empty id"),
                    ToolName::new("read_file").expect("a non-empty name"),
                    "{}".to_owned(),
                )]
            )
            .tool_admission(),
            ToolAdmission::Admitted
        );
    }

    #[test]
    fn a_duplicate_id_is_reported_before_a_length_no_record_could_hold() {
        let oversized = "n".repeat(MAX_CONVERSATION_IDENTITY_BYTES + 1);
        let repeated = ToolCall::local(
            ToolCallId::new(&oversized).expect("a non-empty id"),
            ToolName::new("read_file").expect("a non-empty name"),
            "{}".to_owned(),
        );

        assert_eq!(
            completion(Some(FinishReason::ToolCalls), vec![repeated.clone(), repeated]).tool_admission(),
            ToolAdmission::RejectedDuplicateIdentity
        );
    }

    #[test]
    fn disposition_reads_finish_reason_against_the_payload() {
        let cases = [
            (None, false, Disposition::Interrupted),
            (None, true, Disposition::Interrupted),
            (Some(FinishReason::ProviderError), false, Disposition::ProviderFailure),
            (Some(FinishReason::ProviderError), true, Disposition::ProviderFailure),
            (Some(FinishReason::ContentFilter), false, Disposition::ProviderFailure),
            (Some(FinishReason::ContentFilter), true, Disposition::ProviderFailure),
            (Some(FinishReason::Length), false, Disposition::LengthLimited),
            (Some(FinishReason::Length), true, Disposition::LengthLimited),
            (Some(FinishReason::ToolCalls), true, Disposition::Completed),
            (Some(FinishReason::ToolCalls), false, Disposition::InvalidCompletion),
            (Some(FinishReason::Stop), true, Disposition::InvalidCompletion),
            (Some(FinishReason::Other), true, Disposition::InvalidCompletion),
            (Some(FinishReason::Stop), false, Disposition::Completed),
            (Some(FinishReason::Other), false, Disposition::Completed),
        ];

        for (finish_reason, has_calls, expected) in cases {
            let calls = if has_calls { vec![local("r-call")] } else { Vec::new() };
            assert_eq!(
                completion(finish_reason, calls).disposition(),
                expected,
                "{finish_reason:?} with calls={has_calls}"
            );
        }
    }

    #[test]
    fn a_terminal_stop_is_coherent_only_when_every_call_ran_upstream() {
        assert_eq!(
            completion(Some(FinishReason::Stop), vec![provider_executed("r-provider")]).disposition(),
            Disposition::Completed
        );
        assert_eq!(
            completion(Some(FinishReason::Stop), vec![local("r-local")]).disposition(),
            Disposition::InvalidCompletion
        );
        assert_eq!(
            completion(
                Some(FinishReason::Stop),
                vec![provider_executed("r-provider"), local("r-local")]
            )
            .disposition(),
            Disposition::InvalidCompletion,
            "one local call left to run contradicts a stop"
        );
        assert_eq!(
            completion(Some(FinishReason::Other), vec![provider_executed("r-provider")]).disposition(),
            Disposition::InvalidCompletion,
            "only stop tolerates provider-executed calls"
        );
    }

    #[test]
    fn a_repeated_call_id_rejects_the_completion_whatever_its_provenance() {
        let cases = [
            vec![local("r-duplicate"), local("r-duplicate")],
            vec![provider_executed("r-duplicate"), provider_executed("r-duplicate")],
            vec![provider_executed("r-duplicate"), local("r-duplicate")],
        ];
        for calls in cases {
            assert_eq!(
                completion(None, calls).tool_admission(),
                ToolAdmission::RejectedDuplicateIdentity
            );
        }
        assert_eq!(
            completion(None, vec![local("r-call"), local("t-call")]).tool_admission(),
            ToolAdmission::Admitted
        );
    }

    #[test]
    fn malformed_arguments_are_fatal_only_when_the_provider_already_ran_the_call() {
        let mut local_call = local("r-local");
        local_call.arguments_json = "{".to_string();
        assert_eq!(
            completion(None, vec![local_call]).tool_admission(),
            ToolAdmission::Admitted,
            "dispatch turns this into an error the model can correct"
        );

        let mut provider_call = provider_executed("r-provider");
        provider_call.arguments_json = "{".to_string();
        assert_eq!(
            completion(None, vec![provider_call]).tool_admission(),
            ToolAdmission::RejectedProviderArguments
        );
    }

    #[test]
    fn a_decoder_fault_outranks_every_other_check() {
        let mut with_duplicates = completion(None, vec![local("r-duplicate"), local("r-duplicate")]);
        with_duplicates.provider_result_fault = Some(ProviderResultFault::Ambiguous);
        assert_eq!(
            with_duplicates.tool_admission(),
            ToolAdmission::RejectedProviderResult(ProviderResultFault::Ambiguous),
            "the payload could not be decoded, so nothing later is worth reporting"
        );
    }

    #[test]
    fn dispatch_needs_a_coherent_turn_admissible_calls_and_local_work() {
        assert!(completion(Some(FinishReason::ToolCalls), vec![local("r-call")]).wants_tool_dispatch());
        assert!(
            !completion(Some(FinishReason::ToolCalls), Vec::new()).wants_tool_dispatch(),
            "a promise of calls without any is incoherent"
        );
        assert!(
            !completion(None, vec![local("r-call")]).wants_tool_dispatch(),
            "an interrupted turn is not a turn to continue"
        );
        assert!(
            !completion(Some(FinishReason::ToolCalls), vec![provider_executed("r-provider")]).wants_tool_dispatch(),
            "there is nothing left for trogonai to run"
        );
        assert!(
            !completion(
                Some(FinishReason::ToolCalls),
                vec![local("r-duplicate"), local("r-duplicate")]
            )
            .wants_tool_dispatch(),
            "inadmissible calls are not dispatched"
        );
    }

    #[test]
    fn an_empty_completion_is_admitted_and_interrupted() {
        let empty = ProviderCompletion::default();
        assert_eq!(empty.tool_admission(), ToolAdmission::Admitted);
        assert_eq!(empty.disposition(), Disposition::Interrupted);
        assert!(!empty.all_tool_calls_provider_executed());
    }

    #[test]
    fn a_provider_that_spoke_and_ran_everything_itself_has_ended_the_turn() {
        let mut completion = completion(Some(FinishReason::Stop), vec![provider_executed("a")]);
        completion.content = Some("here is what I found".to_owned());

        assert!(completion.is_terminal_provider_executed());
    }

    #[test]
    fn a_provider_that_ran_everything_but_said_nothing_has_not_ended_the_turn() {
        let mut completion = completion(Some(FinishReason::Stop), vec![provider_executed("a")]);
        completion.content = Some(String::new());

        assert!(!completion.is_terminal_provider_executed());

        completion.content = None;
        assert!(!completion.is_terminal_provider_executed());
    }

    #[test]
    fn a_provider_that_left_a_call_for_trogonai_has_not_ended_the_turn() {
        let mut completion = completion(Some(FinishReason::Stop), vec![local("a")]);
        completion.content = Some("here is what I found".to_owned());

        assert!(!completion.is_terminal_provider_executed());
    }

    #[test]
    fn a_turn_the_provider_did_not_stop_on_has_not_ended_that_way() {
        let mut completion = completion(Some(FinishReason::ToolCalls), vec![provider_executed("a")]);
        completion.content = Some("here is what I found".to_owned());

        assert!(!completion.is_terminal_provider_executed());
    }
}
