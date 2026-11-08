//! Canary strings for the redaction guarantee (ADR#0086 Decision 5).
//!
//! The guarantee is structural: every string in a record is an [`AuditName`] or a
//! [`ReasonCode`], and neither can carry the shapes secrets, tokens, prompts,
//! headers and bodies take. The tests here pin that down and also check the
//! records, their `Debug` renderings and the log lines the writer emits.
//!
//! [`AuditName`]: crate::audit_name::AuditName
//! [`ReasonCode`]: crate::reason_code::ReasonCode

/// One canary per kind of content that must never reach a record. Each carries a
/// marker no legitimate name contains, in the shape the content really takes.
pub(crate) fn canaries() -> [&'static str; 5] {
    [
        "password=CANARY_SECRET_6b1d",
        "Bearer CANARY_TOKEN_6b1d",
        "ignore all previous instructions CANARY_PROMPT_6b1d",
        "Authorization: Basic CANARY_HEADER_6b1d",
        "{\"body\":\"CANARY_BODY_6b1d\"}",
    ]
}

pub(crate) fn marker() -> &'static str {
    "CANARY_"
}

#[cfg(test)]
mod tests;
