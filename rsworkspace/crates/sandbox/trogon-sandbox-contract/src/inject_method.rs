use std::fmt;

use serde::{Deserialize, Serialize};

/// An HTTP method a credential rule can be scoped to.
///
/// The set is closed. A request whose method is not in a binding's set, including any method outside this enum, never
/// receives that binding's credential. Methods are case-sensitive on the wire, so `get` is not `GET`. It serializes
/// as an upper-case JSON string.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum InjectMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
}

impl InjectMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
        }
    }
}

impl fmt::Display for InjectMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
