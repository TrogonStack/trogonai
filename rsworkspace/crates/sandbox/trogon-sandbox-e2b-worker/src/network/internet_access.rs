use serde::Serialize;

/// Whether a sandbox may reach the internet at all, as E2B's `allow_internet_access` flag.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(into = "bool")]
pub enum InternetAccess {
    Allowed,
    Denied,
}

impl From<InternetAccess> for bool {
    fn from(access: InternetAccess) -> Self {
        matches!(access, InternetAccess::Allowed)
    }
}
