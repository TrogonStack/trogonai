/// Mutating session operations that require a Session Lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMutatingOperation {
    PromptTurn,
    SwitchModel,
    Compact,
    Fork,
    Restore,
    Close,
    MutatingToolCall,
}

impl SessionMutatingOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PromptTurn => "prompt_turn",
            Self::SwitchModel => "switch_model",
            Self::Compact => "compact",
            Self::Fork => "fork",
            Self::Restore => "restore",
            Self::Close => "close",
            Self::MutatingToolCall => "mutating_tool_call",
        }
    }
}
