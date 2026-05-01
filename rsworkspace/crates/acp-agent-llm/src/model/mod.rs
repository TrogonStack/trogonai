pub mod anthropic;
pub mod openai;

use crate::error::CompletionError;
use crate::model_id::ModelId;
use crate::provider_name::ProviderName;
use tokio::sync::{mpsc, watch};

/// A single message in a conversation.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

/// Who said it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatRole {
    /// Instructions to the model (e.g., "you are a helpful assistant").
    System,
    /// What the human typed.
    User,
    /// What the LLM replied.
    Assistant,
}

/// Events streamed during a completion.
#[derive(Debug, Clone)]
pub enum CompletionEvent {
    /// A chunk of streamed text.
    Text(String),
    /// The model finished generating.
    Stop(StopReason),
    /// Token usage update (if the provider reports it).
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    Other(String),
}

/// A specific model that can stream completions.
/// One instance per model (claude-opus-4-6, gpt-4o, etc.).
///
/// `?Send` because the ACP Agent trait runs on a single-threaded `LocalSet`.
#[async_trait::async_trait(?Send)]
pub trait LanguageModel {
    fn id(&self) -> &ModelId;
    fn provider_id(&self) -> &ProviderName;
    fn max_token_count(&self) -> u64;

    /// Whether this model supports tool use. Ready for future use.
    fn supports_tools(&self) -> bool {
        false
    }
    /// Whether this model supports image input. Ready for future use.
    fn supports_images(&self) -> bool {
        false
    }
    /// Whether this model supports extended thinking. Ready for future use.
    fn supports_thinking(&self) -> bool {
        false
    }

    /// Stream a completion. Sends `CompletionEvent`s via `event_tx` as they arrive
    /// from the provider's SSE stream.
    ///
    /// Monitors `cancel` — when it becomes `true`, aborts the HTTP request
    /// and returns `CompletionError::Cancelled`.
    async fn stream_completion(
        &self,
        messages: &[ChatMessage],
        event_tx: mpsc::Sender<CompletionEvent>,
        cancel: watch::Receiver<bool>,
    ) -> Result<(), CompletionError>;
}
