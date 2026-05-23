//! NATS push ACL binding — dedicated **gateway User** may publish **`{prefix}.push.>`** where policy allows.
//!
//! ## NSC posture (Phase 0)
//!
//! Per-tenant NATS Accounts isolate subject space; `{prefix}` defaults to `a2a` (tenant `A2A_PREFIX`).
//! Operators provision **User JWTs** with `nsc add user` publish/subscribe flags — the gateway crate does **not**
//! mint credentials; it documents the ACL shape operators must deploy before push remediation or DLQ forwarding
//! can run under a long-lived gateway service identity.
//!
//! | Role | Publish (allow) | Subscribe (allow) | Notes |
//! |------|-----------------|-------------------|-------|
//! | **Caller User** | `{prefix}.gateway.>` | `_INBOX.{caller_id}.>` | Ingress request/reply only; replies on caller-owned inbox. Optional subscribe on `{prefix}.push.{caller_id}.>` for push read path. |
//! | **Gateway User** | `{prefix}.agent.>`, `{prefix}.task.>`, `{prefix}.push.>` | `{prefix}.gateway.>` | Queue-group consumer on ingress; forwards to agents, task events, and push envelopes. **`{prefix}.push.>`** is granted only when policy tables authorize outbound push — not a blanket bypass of Tier 1 checks. |
//!
//! **Policy tables.** Gateway-side push publish is conditional on deployed SpiceDB / Wasmtime policy (see
//! [`A2A_PLAN.md`](../../../../A2A_PLAN.md)). NSC ACLs bound the **maximum** subject surface; runtime policy
//! decides whether a given push envelope may use `{prefix}.push.{caller_id}.>` (or related targets) on each request.
//!
//! ## Operator references
//!
//! - [Subject ACL quick reference](../../../../docs/A2A_SUBJECT_ACL_QUICKREF.md) — one-page Caller / Gateway / Registrar patterns.
//! - [NSC account bootstrap](../../../../docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md) — `nsc add user` templates, JetStream assets, verification steps.
//!
//! Example gateway User provisioning (default prefix):
//!
//! ```text
//! nsc add user -a [TENANT_ACCOUNT] -n a2a-gateway \
//!   --allow-sub "a2a.gateway.>" \
//!   --allow-pub "a2a.agent.>" \
//!   --allow-pub "a2a.task.>" \
//!   --allow-pub "a2a.push.>"
//! ```

/// Default-prefix **`nsc add user --allow-pub`** patterns for the long-lived gateway service User.
///
/// Substitute `a2a` with `{prefix}` when the tenant uses a custom `A2A_PREFIX`. Includes `{prefix}.push.>`
/// alongside agent and task egress — push publish remains subject to runtime policy tables even when NSC allows it.
pub const TEMPLATE_GATEWAY_PUBLISH_ALLOWED: &[&str] = &["a2a.agent.>", "a2a.task.>", "a2a.push.>"];

/// Default-prefix **`nsc add user --allow-sub`** pattern for gateway ingress (queue-group consumer).
pub const TEMPLATE_GATEWAY_SUBSCRIBE_ALLOWED: &[&str] = &["a2a.gateway.>"];

/// Default-prefix caller publish pattern — gateway ingress only.
pub const TEMPLATE_CALLER_PUBLISH_ALLOWED: &[&str] = &["a2a.gateway.>"];

/// Default-prefix caller subscribe pattern — caller-owned reply inbox (`{caller_id}` substituted per User).
pub const TEMPLATE_CALLER_SUBSCRIBE_ALLOWED: &[&str] = &["_INBOX.{caller_id}.>"];

/// Compile-time NSC ACL documentation generator (no file I/O).
///
/// Implementors return static markdown-ish operator text suitable for Rustdoc, runbooks, or future
/// `nsc` export scaffolding. Source of truth remains the docs tree — see cross-links on [`GatewayUserPushAcl`].
pub trait AclStatementGenerator {
    /// Static operator documentation for this ACL binding (embedded at compile time).
    fn acl_statement_docs() -> &'static str;
}

/// Gateway User push + ingress ACL binding (NSC templates, default prefix `a2a`).
pub struct GatewayUserPushAcl;

impl AclStatementGenerator for GatewayUserPushAcl {
    fn acl_statement_docs() -> &'static str {
        r#"Gateway User (a2a-gateway service)

Publish (allow):
  - a2a.agent.>   — forward JSON-RPC to agent subjects
  - a2a.task.>    — task event egress
  - a2a.push.>    — push envelopes when policy tables allow (NSC max surface; runtime policy gates each send)

Subscribe (allow):
  - a2a.gateway.> — queue-group ingress consumer

Caller User (minted by auth callout or static template)

Publish (allow):
  - a2a.gateway.> — request/reply into gateway ingress only

Subscribe (allow):
  - _INBOX.{caller_id}.> — caller-owned reply inbox (stable {caller_id} per minted JWT)

Push publish on a2a.push.> is conditional on policy tables — NSC grants the subject ceiling; gateway policy decides allow/deny per envelope.

See docs/A2A_SUBJECT_ACL_QUICKREF.md and docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md for full Phase 0 ACL tables and nsc add user examples."#
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_publish_template_includes_push_agent_and_task() {
        assert_eq!(
            TEMPLATE_GATEWAY_PUBLISH_ALLOWED,
            &["a2a.agent.>", "a2a.task.>", "a2a.push.>"]
        );
    }

    #[test]
    fn acl_statement_docs_are_non_empty_static() {
        let docs = GatewayUserPushAcl::acl_statement_docs();
        assert!(docs.contains("a2a.push.>"));
        assert!(docs.contains("_INBOX.{caller_id}.>"));
        assert!(docs.contains("A2A_SUBJECT_ACL_QUICKREF.md"));
        assert!(docs.contains("A2A_NSC_ACCOUNT_BOOTSTRAP.md"));
    }
}
