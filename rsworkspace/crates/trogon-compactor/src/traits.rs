use crate::error::CompactorError;
use crate::types::Message;

/// Abstracts the LLM call so `Compactor` can be tested without a real HTTP client.
pub trait LlmProvider: Send + Sync {
    fn generate_summary<'a>(
        &'a self,
        messages: &'a [Message],
        previous_summary: Option<&'a str>,
    ) -> impl std::future::Future<Output = Result<String, CompactorError>> + Send + 'a;
}
