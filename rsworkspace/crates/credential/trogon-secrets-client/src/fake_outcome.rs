use std::time::{Duration, SystemTime};

use trogon_std::SecretString;
use trogonai_proto::secrets::exchange::v1alpha1::ResolveFailureCategory as Category;

use super::fake_answer::FakeAnswer;

/// One answer a [`FakeSecretsService`](crate::FakeSecretsService) gives, in
/// the order it was scripted.
#[derive(Clone, Debug)]
pub struct FakeOutcome {
    delay: Duration,
    answer: Option<FakeAnswer>,
}

impl FakeOutcome {
    /// Releases `value`, sealed to the requester with the fake's own xkey.
    pub fn release(value: SecretString, valid_until: SystemTime, bundle_generation: u64) -> Self {
        Self::answering(FakeAnswer::Release {
            value,
            valid_until,
            bundle_generation,
        })
    }

    pub fn denied() -> Self {
        Self::failure(Category::RESOLVE_FAILURE_CATEGORY_DENIED, None)
    }

    pub fn key_denied() -> Self {
        Self::failure(Category::RESOLVE_FAILURE_CATEGORY_KEY_DENIED, None)
    }

    pub fn unavailable() -> Self {
        Self::failure(Category::RESOLVE_FAILURE_CATEGORY_UNAVAILABLE, None)
    }

    pub fn integrity_failure() -> Self {
        Self::failure(Category::RESOLVE_FAILURE_CATEGORY_INTEGRITY_FAILURE, None)
    }

    pub fn internal() -> Self {
        Self::failure(Category::RESOLVE_FAILURE_CATEGORY_INTERNAL, None)
    }

    pub fn throttled(retry_after: Duration) -> Self {
        Self::failure(Category::RESOLVE_FAILURE_CATEGORY_THROTTLED, Some(retry_after))
    }

    /// Refuses the request itself on the ADR 0016 error channel.
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::answering(FakeAnswer::InvalidArgument(message.into()))
    }

    /// Answers nothing: the request fails as though the bus were unreachable.
    pub fn unreachable() -> Self {
        Self {
            delay: Duration::ZERO,
            answer: None,
        }
    }

    /// Gives this outcome only after `delay`.
    #[must_use = "after returns the outcome by value; script the result"]
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub(crate) const fn delay(&self) -> Duration {
        self.delay
    }

    pub(crate) fn into_answer(self) -> Option<FakeAnswer> {
        self.answer
    }

    fn failure(category: Category, retry_after: Option<Duration>) -> Self {
        Self::answering(FakeAnswer::Failure { category, retry_after })
    }

    fn answering(answer: FakeAnswer) -> Self {
        Self {
            delay: Duration::ZERO,
            answer: Some(answer),
        }
    }
}
