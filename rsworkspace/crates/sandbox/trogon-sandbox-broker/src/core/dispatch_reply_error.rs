use trogon_sandbox_contract::LeaseError;

/// Why a worker's reply to a dispatch gave no usable receipt.
#[derive(Debug, thiserror::Error)]
pub(super) enum DispatchReplyError {
    /// The request did not reach the worker or got no answer.
    #[error("{message}")]
    Transport { message: String },
    /// The worker answered, and the answer is a refusal or cannot be decoded.
    #[error(transparent)]
    Provider(#[from] LeaseError),
}

impl DispatchReplyError {
    /// The provider's answer when the worker replied at all.
    pub(super) fn denial(&self) -> Option<&LeaseError> {
        match self {
            Self::Provider(denial) => Some(denial),
            Self::Transport { .. } => None,
        }
    }

    /// Whether the worker explicitly refused the attempt, which warrants
    /// tearing it down; every other failure leaves the attempt pending.
    pub(super) fn is_refusal(&self) -> bool {
        matches!(
            self.denial(),
            Some(
                LeaseError::ProviderLaunchFailed { .. }
                    | LeaseError::ProviderRejectedDispatch { .. }
                    | LeaseError::InvalidNativeOptions { .. }
            )
        )
    }
}
