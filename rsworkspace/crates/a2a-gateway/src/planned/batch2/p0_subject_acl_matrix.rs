//! Phase 0 — Subject ACL matrix per Account (caller vs gateway Users).
//!
//! Compile-time template rows for Phase 0 per-User NATS subject ACL inside a tenant Account.
//! Substitute `{prefix}` (default `a2a`) and `{caller_id}` when provisioning User JWTs with `nsc add user`.
//!
//! ## Operator references
//!
//! - [Subject ACL quick reference](../../../../docs/A2A_SUBJECT_ACL_QUICKREF.md) — one-page Caller / Gateway / Registrar patterns.
//! - [NSC account bootstrap](../../../../docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md) — `nsc add user` templates, JetStream assets, verification steps.

/// One row in the Phase 0 Account subject ACL matrix (placeholder patterns, not resolved at runtime).
pub struct SubjectAclTemplateRow {
    /// Operator-facing role label (e.g. `Caller User`, `Gateway User`).
    pub role: &'static str,
    /// Comma-separated `nsc add user --allow-pub` patterns for this role.
    pub allow_publish: &'static str,
    /// Comma-separated `nsc add user --allow-sub` patterns for this role.
    pub allow_subscribe: &'static str,
}

/// Expected Phase 0 caller and gateway User ACL bindings inside each tenant Account.
///
/// | Role | Publish (allow) | Subscribe (allow) |
/// |------|-----------------|-------------------|
/// | **Caller User** | `{prefix}.gateway.>` | `_INBOX.{caller_id}.>` |
/// | **Gateway User** | `{prefix}.agent.>`, `{prefix}.task.>`, `{prefix}.push.>` | `{prefix}.gateway.>` |
pub const EXPECTED_ACCOUNT_ACL_ROWS: &[SubjectAclTemplateRow] = &[
    SubjectAclTemplateRow {
        role: "Caller User",
        allow_publish: "{prefix}.gateway.>",
        allow_subscribe: "_INBOX.{caller_id}.>",
    },
    SubjectAclTemplateRow {
        role: "Gateway User",
        allow_publish: "{prefix}.agent.>, {prefix}.task.>, {prefix}.push.>",
        allow_subscribe: "{prefix}.gateway.>",
    },
];

/// Returns the Phase 0 caller/gateway Account ACL template rows.
pub fn templates() -> &'static [SubjectAclTemplateRow] {
    EXPECTED_ACCOUNT_ACL_ROWS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_returns_expected_account_acl_rows() {
        let rows = templates();
        assert_eq!(rows.len(), 2);

        assert_eq!(rows[0].role, "Caller User");
        assert_eq!(rows[0].allow_publish, "{prefix}.gateway.>");
        assert_eq!(rows[0].allow_subscribe, "_INBOX.{caller_id}.>");

        assert_eq!(rows[1].role, "Gateway User");
        assert_eq!(
            rows[1].allow_publish,
            "{prefix}.agent.>, {prefix}.task.>, {prefix}.push.>"
        );
        assert_eq!(rows[1].allow_subscribe, "{prefix}.gateway.>");
    }
}
