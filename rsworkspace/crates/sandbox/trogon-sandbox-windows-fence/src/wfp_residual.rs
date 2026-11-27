/// A gap the verification observed that the filter set does not close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WfpResidual {
    AnonymousEgress,
    AnonymousLoopback,
    DenyOnlyUserSid,
    LoweredIntegrityUnprobed,
}

impl WfpResidual {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "anonymous-egress" => Some(Self::AnonymousEgress),
            "anonymous-loopback" => Some(Self::AnonymousLoopback),
            "deny-only-user-sid" => Some(Self::DenyOnlyUserSid),
            "lowered-integrity-unprobed" => Some(Self::LoweredIntegrityUnprobed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnonymousEgress => "anonymous-egress",
            Self::AnonymousLoopback => "anonymous-loopback",
            Self::DenyOnlyUserSid => "deny-only-user-sid",
            Self::LoweredIntegrityUnprobed => "lowered-integrity-unprobed",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Self::AnonymousEgress => {
                "a connect to an off-loopback destination made while impersonating the anonymous \
                 token is not blocked (no filter is keyed on that token; a sandboxed process that \
                 puts it on a thread before creating a socket is not fenced)"
            }
            Self::AnonymousLoopback => {
                "a connect to a loopback destination made while impersonating the anonymous token \
                 is not blocked (local listeners outside the proxy port range are reachable that \
                 way)"
            }
            Self::DenyOnlyUserSid => {
                "a connect made under a copy of the sandbox token whose user SID is deny-only is \
                 not blocked (the filters cannot match that token)"
            }
            Self::LoweredIntegrityUnprobed => {
                "a lowered-integrity copy of the sandbox token could not be set up on this \
                 machine, so no connect under it was observed"
            }
        }
    }
}
