//! How a model turn ended.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{TurnTokens, UnixMillis};

/// Provider-reported reason a stream stopped.
///
/// There is deliberately no variant for a reason the runtime does not recognize: an unknown string
/// parses to `None`, and an absent finish reason means the turn was interrupted rather than
/// finished. Keeping an unrecognized reason would make it terminal by default, which reads a
/// provider the runtime does not understand as a clean stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// The model finished its response.
    Stop,
    /// The model requested tools and expects their results.
    ToolCalls,
    /// Output hit the token ceiling.
    Length,
    /// The provider refused to continue.
    ContentFilter,
    /// The provider reported an error of its own.
    ProviderError,
    /// The provider reported a reason inside its domain that means none of the
    /// above.
    Other,
}

impl FinishReason {
    /// Parses the gateway's unified vocabulary.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "stop" => Some(Self::Stop),
            "length" => Some(Self::Length),
            "content-filter" => Some(Self::ContentFilter),
            "tool-calls" => Some(Self::ToolCalls),
            "error" => Some(Self::ProviderError),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    /// Parses the unified vocabulary plus the two snake_case spellings older
    /// providers still send.
    #[must_use]
    pub fn parse_legacy(raw: &str) -> Option<Self> {
        match raw {
            "tool_calls" => Some(Self::ToolCalls),
            "content_filter" => Some(Self::ContentFilter),
            _ => Self::parse(raw),
        }
    }

    /// The unified spelling, which [`Self::parse`] accepts back.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Length => "length",
            Self::ContentFilter => "content-filter",
            Self::ToolCalls => "tool-calls",
            Self::ProviderError => "error",
            Self::Other => "other",
        }
    }

    /// Whether the agent loop should run tools and continue.
    #[must_use]
    pub fn expects_tool_results(self) -> bool {
        matches!(self, Self::ToolCalls)
    }

    /// Whether the turn ended without more work to do.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        !self.expects_tool_results()
    }
}

/// Aggregate accounting for one completed turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSummary {
    /// When the turn was submitted.
    ///
    /// Kept beside the duration rather than derived from it: a transcript
    /// replayed later stamps its entries with these, and a duration says how
    /// long the turn took without saying when it happened.
    pub started_at: Option<UnixMillis>,
    /// When the turn finished.
    pub completed_at: Option<UnixMillis>,
    /// Wall time from request to completion.
    pub duration: Duration,
    /// Time the turn spent waiting for the model to start answering.
    ///
    /// Summed across the turn's gateway calls rather than measured once: a turn
    /// that made three calls waited three times, and what a person watching a
    /// spinner wants to know is how long the spinner was up.
    pub thinking: Duration,
    /// Tokens attributed to this turn.
    pub tokens: TurnTokens,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tool_calls_continue_the_loop() {
        assert!(FinishReason::ToolCalls.expects_tool_results());
        assert!(FinishReason::Stop.is_terminal());
        assert!(FinishReason::Other.is_terminal());
        assert!(FinishReason::ProviderError.is_terminal());
    }

    #[test]
    fn every_reason_round_trips_through_its_label() {
        for reason in [
            FinishReason::Stop,
            FinishReason::Length,
            FinishReason::ContentFilter,
            FinishReason::ToolCalls,
            FinishReason::ProviderError,
            FinishReason::Other,
        ] {
            assert_eq!(FinishReason::parse(reason.label()), Some(reason));
        }
    }

    #[test]
    fn a_reason_outside_the_domain_does_not_parse() {
        for raw in ["", "future-reason", "Stop", "tool_calls", "content_filter"] {
            assert_eq!(FinishReason::parse(raw), None, "{raw}");
        }
    }

    #[test]
    fn the_legacy_spellings_are_accepted_only_by_the_legacy_parser() {
        assert_eq!(FinishReason::parse_legacy("tool_calls"), Some(FinishReason::ToolCalls));
        assert_eq!(
            FinishReason::parse_legacy("content_filter"),
            Some(FinishReason::ContentFilter)
        );
        assert_eq!(
            FinishReason::parse_legacy("stop"),
            Some(FinishReason::Stop),
            "the legacy parser is a superset"
        );
        assert_eq!(FinishReason::parse_legacy("future-reason"), None);
    }
}
