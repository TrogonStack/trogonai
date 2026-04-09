use crate::model::ChatMessage;
use crate::model_id::ModelId;
use crate::provider_name::ProviderName;
use tokio::sync::watch;

pub struct Session {
    provider_name: ProviderName,
    model_id: ModelId,
    messages: Vec<ChatMessage>,
    cancel_tx: watch::Sender<bool>,
    cancel_rx: watch::Receiver<bool>,
}

impl Session {
    pub fn new(provider_name: ProviderName, model_id: ModelId) -> Self {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        Self {
            provider_name,
            model_id,
            messages: Vec::new(),
            cancel_tx,
            cancel_rx,
        }
    }

    pub fn provider_name(&self) -> &ProviderName {
        &self.provider_name
    }
    pub fn model_id(&self) -> &ModelId {
        &self.model_id
    }
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }
    pub fn push_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }

    pub fn set_model(&mut self, model: ModelId) {
        self.model_id = model;
    }
    pub fn set_provider(&mut self, provider: ProviderName) {
        self.provider_name = provider;
    }

    /// Get a cancel receiver for the current prompt.
    pub fn cancel_rx(&self) -> watch::Receiver<bool> {
        self.cancel_rx.clone()
    }

    /// Signal cancellation of the active prompt.
    pub fn cancel(&self) {
        let _ = self.cancel_tx.send(true);
    }

    /// Reset the cancel signal for the next prompt.
    pub fn reset_cancel(&self) {
        let _ = self.cancel_tx.send(false);
    }

    /// Clone history into a new session (for fork).
    pub fn fork(&self, provider: ProviderName, model: ModelId) -> Self {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        Self {
            provider_name: provider,
            model_id: model,
            messages: self.messages.clone(),
            cancel_tx,
            cancel_rx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChatRole;

    #[test]
    fn new_session_starts_empty() {
        let session = Session::new(
            ProviderName::new("anthropic").unwrap(),
            ModelId::new("claude-sonnet-4-6").unwrap(),
        );
        assert!(session.messages().is_empty());
        assert_eq!(session.provider_name().as_str(), "anthropic");
        assert_eq!(session.model_id().as_str(), "claude-sonnet-4-6");
    }

    #[test]
    fn push_message_appends() {
        let mut session = Session::new(
            ProviderName::new("anthropic").unwrap(),
            ModelId::new("test").unwrap(),
        );
        session.push_message(ChatMessage {
            role: ChatRole::User,
            content: "hello".to_string(),
        });
        assert_eq!(session.messages().len(), 1);
    }

    #[test]
    fn fork_clones_history() {
        let mut session = Session::new(
            ProviderName::new("anthropic").unwrap(),
            ModelId::new("test").unwrap(),
        );
        session.push_message(ChatMessage {
            role: ChatRole::User,
            content: "hello".to_string(),
        });
        let forked = session.fork(
            ProviderName::new("openai").unwrap(),
            ModelId::new("gpt-4o").unwrap(),
        );
        assert_eq!(forked.messages().len(), 1);
        assert_eq!(forked.provider_name().as_str(), "openai");
        assert_eq!(forked.model_id().as_str(), "gpt-4o");
    }
}
