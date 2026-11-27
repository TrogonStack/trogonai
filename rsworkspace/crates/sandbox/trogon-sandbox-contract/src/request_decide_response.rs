use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Reply to a [`RequestDecideRequest`](crate::RequestDecideRequest), tagged by `verdict`.
///
/// Anything that does not parse, or breaks a limit of the egress proxy's decider protocol, is treated by the proxy as a deny.
/// `Debug` lists the names of the headers an `Allow` sets and leaves out their values and the `credential` field,
/// so a reply that carries a secret stays out of logs.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict", deny_unknown_fields)]
pub enum RequestDecideResponse {
    Allow {
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        set_headers: BTreeMap<String, Vec<String>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove_headers: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cred: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential: Option<String>,
    },
    Deny {
        status: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    NeedBody {
        max: u64,
    },
}

impl fmt::Debug for RequestDecideResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allow {
                set_headers,
                remove_headers,
                cred,
                ..
            } => formatter
                .debug_struct("Allow")
                .field("set_headers", &set_headers.keys().collect::<Vec<_>>())
                .field("remove_headers", remove_headers)
                .field("cred", cred)
                .finish_non_exhaustive(),
            Self::Deny { status, reason } => formatter
                .debug_struct("Deny")
                .field("status", status)
                .field("reason", reason)
                .finish(),
            Self::NeedBody { max } => formatter.debug_struct("NeedBody").field("max", max).finish(),
        }
    }
}
