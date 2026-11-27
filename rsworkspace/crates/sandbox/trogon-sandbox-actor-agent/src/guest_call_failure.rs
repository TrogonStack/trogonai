use crate::{GuestProtocolError, PROTOCOL_REFUSED_STATUS, UNIMPLEMENTED_STATUS};

/// What a worker saw when it called the agent, reduced to what decides whether an older path may be tried.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum GuestCallFailure {
    /// The agent refused on version grounds: no fallback can help.
    Refused(GuestProtocolError),
    /// The agent said it does not implement the verb.
    Unimplemented,
    /// Any other non-success reply. The call may or may not have run.
    UnexpectedReply { status: u16 },
}

impl GuestCallFailure {
    /// Classifies a non-success HTTP reply.
    ///
    /// Only an explicit [`UNIMPLEMENTED_STATUS`], or a typed [`GuestProtocolError::Unimplemented`] body, counts as
    /// unimplemented; a bare 404 does not, because an old agent proxies unknown paths to the tenant's own port and a
    /// 404 there says nothing about the agent.
    pub fn from_http(status: u16, body: &[u8]) -> Self {
        let typed = serde_json::from_slice::<GuestProtocolError>(body).ok();
        match (status, typed) {
            (_, Some(GuestProtocolError::Unimplemented)) => Self::Unimplemented,
            (PROTOCOL_REFUSED_STATUS, Some(error)) => Self::Refused(error),
            (UNIMPLEMENTED_STATUS, _) => Self::Unimplemented,
            _ => Self::UnexpectedReply { status },
        }
    }

    /// Whether the caller may retry on a legacy path.
    ///
    /// It is never true for an unexpected reply: the first attempt may already have run, and a fallback could run it
    /// twice.
    pub fn may_fall_back(&self) -> bool {
        matches!(self, Self::Unimplemented)
    }
}
