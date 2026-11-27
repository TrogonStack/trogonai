use serde::Deserialize;
use serde_json::Value;

use crate::OperationState;

/// A SPACE operation: the asynchronous outcome of a create, delete or lifecycle call.
#[derive(Debug, Clone, Deserialize)]
pub struct Operation {
    #[serde(default)]
    id: String,
    state: OperationState,
    #[serde(default)]
    terminal_error_code: Option<String>,
    #[serde(default)]
    terminal_error: Option<Value>,
    #[serde(default)]
    progress_message: Option<String>,
}

impl Operation {
    /// The operation's state.
    pub fn state(&self) -> &OperationState {
        &self.state
    }

    /// The operation will not change state again.
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// The operation finished and took effect.
    pub fn succeeded(&self) -> bool {
        self.state == OperationState::Succeeded
    }

    /// A short description of why a terminal operation did not succeed.
    pub fn failure(&self) -> String {
        let code = self.terminal_error_code.as_deref().unwrap_or("none");
        let message = self
            .terminal_error
            .as_ref()
            .and_then(|error| error["message"].as_str())
            .or(self.progress_message.as_deref())
            .unwrap_or("");
        format!("operation {} ended {} ({code}) {message}", self.id, self.state)
    }
}
