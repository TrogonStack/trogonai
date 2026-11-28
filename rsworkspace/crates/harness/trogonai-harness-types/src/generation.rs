//! What a gateway reports about one completed generation.

use serde::{Deserialize, Serialize};

use crate::gateway::{GatewayTimestamp, GenerationId};
use crate::id::ModelId;

/// What a generation cost, in the gateway's currency.
///
/// Non-negative and finite by construction.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct GenerationCost(f64);

impl GenerationCost {
    /// Nothing was charged.
    pub const FREE: Self = Self(0.0);

    /// Wraps a finite, non-negative amount.
    #[must_use]
    pub fn new(amount: f64) -> Option<Self> {
        (amount.is_finite() && amount >= 0.0).then_some(Self(amount))
    }

    /// The amount.
    #[must_use]
    pub const fn amount(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for GenerationCost {
    type Error = NotACost;

    fn try_from(amount: f64) -> Result<Self, Self::Error> {
        Self::new(amount).ok_or(NotACost(amount))
    }
}

impl From<GenerationCost> for f64 {
    fn from(cost: GenerationCost) -> Self {
        cost.0
    }
}

/// A rejected cost.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("{0} is not a cost: a cost is finite and not negative")]
pub struct NotACost(pub f64);

/// The token counts behind a generation.
///
/// Holds what the gateway reported, and derives the totals rather than storing them beside
/// their parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationTokens {
    prompt: u64,
    completion: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: Option<u64>,
}

impl GenerationTokens {
    /// The counts a gateway reported, refused when a total would not fit.
    ///
    /// The overflow check happens once, here, rather than at each site that adds
    /// the parts up.
    #[must_use]
    pub fn new(
        prompt: u64,
        completion: u64,
        cache_read: u64,
        cache_write: u64,
        reasoning: Option<u64>,
    ) -> Option<Self> {
        prompt.checked_add(cache_read)?.checked_add(cache_write)?;
        completion.checked_add(reasoning.unwrap_or(0))?;
        Some(Self {
            prompt,
            completion,
            cache_read,
            cache_write,
            reasoning,
        })
    }

    /// Everything the model was charged for reading.
    ///
    /// Cached tokens count toward the input total.
    #[must_use]
    pub const fn input(&self) -> u64 {
        self.prompt + self.cache_read + self.cache_write
    }

    /// Everything the model was charged for writing, reasoning included.
    #[must_use]
    pub const fn output(&self) -> u64 {
        match self.reasoning {
            Some(reasoning) => self.completion + reasoning,
            None => self.completion,
        }
    }

    /// The prompt, exclusive of anything served from cache.
    #[must_use]
    pub const fn prompt(&self) -> u64 {
        self.prompt
    }

    /// The completion, exclusive of reasoning.
    #[must_use]
    pub const fn completion(&self) -> u64 {
        self.completion
    }

    /// Tokens served from cache rather than re-read.
    #[must_use]
    pub const fn cache_read(&self) -> u64 {
        self.cache_read
    }

    /// Tokens written into the cache.
    #[must_use]
    pub const fn cache_write(&self) -> u64 {
        self.cache_write
    }

    /// Reasoning tokens, when the gateway said anything about them.
    ///
    /// `None` is a gateway that did not report the field, which is not the same
    /// as a model that did no reasoning.
    #[must_use]
    pub const fn reasoning(&self) -> Option<u64> {
        self.reasoning
    }
}

/// The gateway's account of one generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationRecord {
    /// Which generation this accounts for.
    pub id: GenerationId,
    /// When the gateway created it.
    pub created_at: GatewayTimestamp,
    /// The model that served it.
    pub model: ModelId,
    /// What it cost.
    pub cost: GenerationCost,
    /// What it spent.
    pub tokens: GenerationTokens,
    /// How many web searches were billed against it.
    pub billable_web_search_calls: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_or_infinite_amount_is_not_a_cost() {
        assert_eq!(GenerationCost::new(-1.0), None);
        assert_eq!(GenerationCost::new(f64::INFINITY), None);
        assert_eq!(GenerationCost::new(f64::NAN), None);
        assert_eq!(GenerationCost::new(0.0), Some(GenerationCost::FREE));
    }

    #[test]
    fn cached_tokens_count_toward_the_input_total() {
        let tokens = GenerationTokens::new(11, 4, 15513, 0, Some(3)).expect("in range");
        assert_eq!(tokens.input(), 15524);
        assert_eq!(tokens.output(), 7);
    }

    #[test]
    fn a_total_that_would_not_fit_is_refused_rather_than_wrapped() {
        assert_eq!(GenerationTokens::new(u64::MAX, 0, 1, 0, None), None);
        assert_eq!(GenerationTokens::new(0, u64::MAX, 0, 0, Some(1)), None);
    }

    #[test]
    fn a_gateway_that_said_nothing_about_reasoning_is_told_apart_from_zero() {
        let silent = GenerationTokens::new(1, 2, 0, 0, None).expect("in range");
        let reported = GenerationTokens::new(1, 2, 0, 0, Some(0)).expect("in range");
        assert_eq!(silent.reasoning(), None);
        assert_eq!(reported.reasoning(), Some(0));
        assert_eq!(silent.output(), reported.output());
    }
}
