use super::{InternetAccess, NetworkRules};

/// The settings sent on sandbox create.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct E2bNetwork {
    pub allow_internet_access: Option<InternetAccess>,
    pub network: Option<NetworkRules>,
}

impl E2bNetwork {
    /// No egress at all.
    pub(super) fn denied() -> Self {
        Self {
            allow_internet_access: Some(InternetAccess::Denied),
            network: None,
        }
    }
}
