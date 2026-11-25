/// Which layer refused a request and why, as the closed vocabulary the gateway
/// owns (ADR#0086 Decision 5). The audit reason code is never the free text of
/// the reply, which can echo content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DenyReason {
    /// The forwarded caller JWT does not prove the principal of the subject.
    CallerJwtRejected,
    /// Tier-1 SpiceDB denied, or could not decide.
    Tier1Spicedb,
    Tier1Declarative,
    Tier2,
    /// A Tier-3 skill refused part redaction.
    Tier3Refused,
    Tier3EngineError,
    /// The payload was not JSON, so Tier-3 could not run and the gateway failed closed.
    Tier3PayloadInvalid,
}

impl DenyReason {
    /// The lower snake case code the record carries.
    pub fn code(self) -> &'static str {
        match self {
            Self::CallerJwtRejected => "caller_jwt_rejected",
            Self::Tier1Spicedb => "tier1_spicedb_denied",
            Self::Tier1Declarative => "tier1_declarative_denied",
            Self::Tier2 => "tier2_denied",
            Self::Tier3Refused => "tier3_refused",
            Self::Tier3EngineError => "tier3_engine_error",
            Self::Tier3PayloadInvalid => "tier3_payload_invalid",
        }
    }

    /// The layer whose verdict this is.
    pub fn layer(self) -> &'static str {
        match self {
            Self::CallerJwtRejected => "caller_jwt",
            Self::Tier1Spicedb => "tier1_spicedb",
            Self::Tier1Declarative => "tier1_declarative",
            Self::Tier2 => "tier2",
            Self::Tier3Refused | Self::Tier3EngineError | Self::Tier3PayloadInvalid => "tier3",
        }
    }
}

#[cfg(test)]
mod tests;
