//! Token accounting.

use serde::{Deserialize, Serialize};

/// A token count that remembers whether the provider vouched for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenCount {
    /// Counted locally, before the provider reported usage.
    Estimated(u64),
    /// Reported by the provider.
    Exact(u64),
}

impl TokenCount {
    /// Zero tokens, not yet corrected by the provider.
    pub const ZERO: Self = Self::Estimated(0);

    /// The count, regardless of provenance.
    #[must_use]
    pub fn value(self) -> u64 {
        match self {
            Self::Estimated(n) | Self::Exact(n) => n,
        }
    }

    /// Whether the provider reported this count.
    #[must_use]
    pub fn is_exact(self) -> bool {
        matches!(self, Self::Exact(_))
    }

    /// Adds tokens, degrading to estimated when either side is an estimate.
    #[must_use]
    pub fn saturating_add(self, other: Self) -> Self {
        let total = self.value().saturating_add(other.value());
        if self.is_exact() && other.is_exact() {
            Self::Exact(total)
        } else {
            Self::Estimated(total)
        }
    }

    /// Replaces an estimate with a provider-reported count.
    #[must_use]
    pub fn confirmed(self, exact: u64) -> Self {
        let _ = self;
        Self::Exact(exact)
    }
}

impl Default for TokenCount {
    fn default() -> Self {
        Self::ZERO
    }
}

/// Absolute per-turn token totals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnTokens {
    /// Prompt tokens submitted this turn.
    pub input: TokenCount,
    /// Tokens generated this turn.
    pub output: TokenCount,
}

impl TurnTokens {
    /// Sums two turns.
    #[must_use]
    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
        }
    }
}

/// What a provider said it spent, as far as it said anything.
///
/// Any count can be missing, and a missing count is not a zero: a provider
/// that reports no output tokens has not told the runtime the response was free.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUsage {
    /// Prompt tokens the provider counted.
    pub input_tokens: Option<u64>,
    /// Generated tokens the provider counted.
    pub output_tokens: Option<u64>,
    /// Prompt tokens the provider served from its cache.
    pub cache_read_tokens: Option<u64>,
    /// Prompt tokens the provider wrote into its cache.
    pub cache_write_tokens: Option<u64>,
    /// Tokens spent reasoning, when the provider separates them.
    pub reasoning_tokens: Option<u64>,
    /// What the provider says it charged, in millionths of a US dollar.
    ///
    /// Only a provider that reports the amount it billed sets this; an
    /// estimate from a price table is never stored here. Whole millionths
    /// keep the report exact and comparable. Absent from older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charged_micros: Option<u64>,
}

impl ProviderUsage {
    /// Whether the provider counted the turn at all.
    ///
    /// Only the prompt and output halves answer that. Cache and reasoning
    /// counts refine a turn the provider already counted; on their own they
    /// leave the turn's size unstated.
    #[must_use]
    pub const fn is_reported(self) -> bool {
        self.input_tokens.is_some() || self.output_tokens.is_some()
    }

    /// Adds another report to this one, saturating on overflow.
    ///
    /// A count the other report leaves out keeps whatever this one had, so a
    /// half nobody reported stays missing rather than turning into a zero.
    pub fn observe(&mut self, usage: Self) {
        add_optional(&mut self.input_tokens, usage.input_tokens);
        add_optional(&mut self.output_tokens, usage.output_tokens);
        add_optional(&mut self.cache_read_tokens, usage.cache_read_tokens);
        add_optional(&mut self.cache_write_tokens, usage.cache_write_tokens);
        add_optional(&mut self.reasoning_tokens, usage.reasoning_tokens);
        add_optional(&mut self.charged_micros, usage.charged_micros);
    }

    /// The same report without the charged amount: what a session's reply
    /// receipts keep, since their format holds token counts only and the
    /// charge is recorded where spend is shown.
    #[must_use]
    pub const fn without_charge(self) -> Self {
        Self {
            charged_micros: None,
            ..self
        }
    }

    /// The turn's totals with whichever halves the provider confirmed.
    ///
    /// A half the provider did not report keeps the estimate it already had,
    /// which is the only honest answer for a count nobody counted.
    #[must_use]
    pub fn confirming(self, tokens: TurnTokens) -> TurnTokens {
        TurnTokens {
            input: self
                .input_tokens
                .map_or(tokens.input, |exact| tokens.input.confirmed(exact)),
            output: self
                .output_tokens
                .map_or(tokens.output, |exact| tokens.output.confirmed(exact)),
        }
    }
}

fn add_optional(total: &mut Option<u64>, value: Option<u64>) {
    let Some(amount) = value else {
        return;
    };
    *total = Some(total.unwrap_or(0).saturating_add(amount));
}

/// What one provider-run tool spent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolUsage {
    /// Prompt tokens the tool consumed.
    pub input_tokens: u64,
    /// Tokens the tool generated.
    pub output_tokens: u64,
    /// Web searches the tool billed for.
    pub web_search_requests: u32,
}

impl ToolUsage {
    /// Sums two tools' usage.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            web_search_requests: self.web_search_requests.saturating_add(other.web_search_requests),
        }
    }
}

/// Exact usage metadata returned by a completed provider stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Billing {
    /// When the gateway recorded the charge.
    ///
    /// The field keeps its wire name because the gateway sends it; the type does
    /// not repeat the unit, since a `GatewayTimestamp` is milliseconds already and
    /// a name saying so invites arithmetic on a raw `i64`.
    #[serde(rename = "created_at_ms")]
    pub created_at: crate::GatewayTimestamp,
    /// Provider-reported model that served the request.
    pub model: crate::ModelId,
    /// Cost in USD.
    pub total_cost: f64,
    /// Prompt tokens.
    pub input_tokens: u64,
    /// Generated tokens.
    pub output_tokens: u64,
    /// Tokens served from the prompt cache.
    pub cache_read_tokens: u64,
    /// Tokens written to the prompt cache.
    pub cache_write_tokens: u64,
    /// Reasoning tokens, when the provider separates them.
    pub reasoning_tokens: Option<u64>,
    /// Billable provider-side web searches.
    pub billable_web_search_calls: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_the_provider_did_not_report_keeps_the_estimate_it_had() {
        let estimated = TurnTokens {
            input: TokenCount::Estimated(100),
            output: TokenCount::Estimated(40),
        };
        let partial = ProviderUsage {
            input_tokens: None,
            output_tokens: Some(37),
            ..ProviderUsage::default()
        };

        let confirmed = partial.confirming(estimated);
        assert_eq!(confirmed.input, TokenCount::Estimated(100));
        assert!(confirmed.output.is_exact());
        assert_eq!(confirmed.output.value(), 37);
    }

    #[test]
    fn a_provider_that_reported_nothing_is_not_a_provider_that_reported_zero() {
        let silent = ProviderUsage::default();
        assert!(!silent.is_reported());
        assert!(
            ProviderUsage {
                input_tokens: Some(0),
                output_tokens: None,
                ..ProviderUsage::default()
            }
            .is_reported()
        );
    }

    #[test]
    fn a_cache_count_refines_a_counted_turn_rather_than_counting_one() {
        let cache_only = ProviderUsage {
            cache_read_tokens: Some(2_048),
            cache_write_tokens: Some(512),
            reasoning_tokens: Some(64),
            ..ProviderUsage::default()
        };

        assert!(!cache_only.is_reported());
        assert_eq!(
            cache_only.confirming(TurnTokens {
                input: TokenCount::Estimated(100),
                output: TokenCount::Estimated(40),
            }),
            TurnTokens {
                input: TokenCount::Estimated(100),
                output: TokenCount::Estimated(40),
            }
        );
    }

    #[test]
    fn tool_usage_adds_up_across_the_tools_of_a_turn() {
        let search = ToolUsage {
            input_tokens: 10,
            output_tokens: 5,
            web_search_requests: 2,
        };
        let fetch = ToolUsage {
            input_tokens: 3,
            output_tokens: 1,
            web_search_requests: 0,
        };

        let total = search.saturating_add(fetch);
        assert_eq!(total.input_tokens, 13);
        assert_eq!(total.web_search_requests, 2);
    }

    #[test]
    fn estimate_poisons_the_sum() {
        let sum = TokenCount::Exact(10).saturating_add(TokenCount::Estimated(5));
        assert_eq!(sum, TokenCount::Estimated(15));
    }

    #[test]
    fn exact_pair_stays_exact() {
        let sum = TokenCount::Exact(10).saturating_add(TokenCount::Exact(5));
        assert_eq!(sum, TokenCount::Exact(15));
    }

    #[test]
    fn confirmation_overrides_an_estimate() {
        assert_eq!(TokenCount::Estimated(99).confirmed(12), TokenCount::Exact(12));
    }
}
