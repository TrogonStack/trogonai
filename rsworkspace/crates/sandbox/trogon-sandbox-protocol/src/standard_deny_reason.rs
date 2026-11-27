use trogon_sandbox_contract::{DenyReason, DenyReasonError};

/// The reasons the proxy gives for a refusal when no operator text applies.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum StandardDenyReason {
    /// A denied domain matched and carries no text of its own.
    OnDenyList,
    /// No allow rule lists the destination.
    NotOnAllowList,
    /// The decider refused the destination.
    UserDenied,
    /// The target could not be read as a host.
    MalformedHost,
}

impl StandardDenyReason {
    /// The words of the reason.
    pub const fn text(self) -> &'static str {
        match self {
            Self::OnDenyList => "host is on the deny list",
            Self::NotOnAllowList => "host is not on the allow list",
            Self::UserDenied => "user denied",
            Self::MalformedHost => "malformed host",
        }
    }

    /// The reason as a [`DenyReason`].
    ///
    /// # Errors
    ///
    /// Returns the error of [`DenyReason::new`], which never fires for the fixed texts.
    pub fn reason(self) -> Result<DenyReason, DenyReasonError> {
        DenyReason::new(self.text())
    }
}
