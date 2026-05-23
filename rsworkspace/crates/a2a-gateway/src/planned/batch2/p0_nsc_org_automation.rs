//! Phase 0 — NSC automation beyond bootstrap (org tooling, scripted exports).
//!
//! Extends the manual operator runbook with compile-time seams for org-scale `nsc export` bundles,
//! JWT push targets, and staged provisioning — **doc-only placeholders** (no subprocess / file I/O).
//!
//! **Related:**
//! - [NSC account bootstrap runbook](../../../../docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md) — manual outline this module extends.
//! - [A2A TODO](../../../../A2A_TODO.md) — Phase 0 checklist; org automation + scripted `nsc` exports remain future.

/// Stages after the manual bootstrap runbook — org-scale provisioning pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisioningStage {
    /// Create tenant Account JWT, JetStream limits, and initial service Users (`nsc add account`, `nsc add user`).
    ScaffoldAccount,
    /// Create `A2A_EVENTS`, `A2A_AGENT_CARDS`, and `A2A_PUSH_DLQ` inside the tenant Account.
    WireJetStream,
    /// Rotate compromised or expired User credentials and re-push JWTs (`nsc push`).
    RotateUsers,
}

impl ProvisioningStage {
    /// Short operator label for runbook tables and future telemetry.
    pub const fn label(self) -> &'static str {
        match self {
            Self::ScaffoldAccount => "scaffold_account",
            Self::WireJetStream => "wire_jetstream",
            Self::RotateUsers => "rotate_users",
        }
    }
}

/// Layout of an org-managed `nsc export` bundle (JWT artifacts + resolver hints).
///
/// Source of truth for directory semantics remains
/// [`A2A_NSC_ACCOUNT_BOOTSTRAP.md`](../../../../docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md); this struct
/// documents the expected on-disk shape for future scripted exports.
pub struct ScriptedExportBundle {
    /// Static description of expected on-disk layout (no file I/O).
    pub dir_layout: &'static str,
}

impl ScriptedExportBundle {
    /// Default org export tree — accounts/, users/, and resolver metadata per tenant Account.
    pub const ORG_DEFAULT: Self = Self {
        dir_layout: r#"org-nsc-export/
  accounts/
    [TENANT_ACCOUNT]/
      account.jwt
      users/
        a2a-gateway.creds
        a2a-registrar.creds
        a2a-caller-template.creds
  resolver/
    operator-url.txt
    account-jwt-import.json
"#,
    };
}

/// Compile-time pipeline description for scripted org `nsc` exports (beyond manual bootstrap).
///
/// Implementors return static operator text suitable for Rustdoc, CI runbooks, or future export
/// scaffolding. No `std::process` invocations — operators execute `nsc` commands manually or via
/// org CI until automation lands ([`A2A_TODO.md`](../../../../A2A_TODO.md)).
pub trait OrgNscExportPipeline {
    /// Operator-facing step list (embedded at compile time).
    fn describe_steps(&self) -> &'static str;
}

/// URI and `nsc push` target documentation for JWT publication to the operator cluster.
pub trait NscJwtPushTarget {
    /// Static docs for resolver / push destination configuration.
    fn push_uri_docs(&self) -> &'static str;
}

/// Default pipeline stub referencing bootstrap extensions tracked in [`A2A_TODO.md`](../../../../A2A_TODO.md).
pub struct OrgNscAutomationRunbook;

impl OrgNscExportPipeline for OrgNscAutomationRunbook {
    fn describe_steps(&self) -> &'static str {
        r#"Org NSC export pipeline (beyond bootstrap runbook)

Stage: scaffold_account
  1. Select operator context (nsc env -o [OPERATOR]).
  2. Add tenant Account if missing (nsc add account -n [TENANT_ACCOUNT]).
  3. Enable JetStream limits (nsc edit account --js-* flags).
  4. Add gateway, registrar, and caller template Users per ACL table.

Stage: wire_jetstream
  5. Push Account and User JWTs (nsc push -a [TENANT_ACCOUNT]).
  6. Bootstrap A2A_EVENTS, A2A_AGENT_CARDS KV, A2A_PUSH_DLQ inside the Account.
  7. Verify gateway / registrar / caller connectivity within ACL bounds.

Stage: rotate_users
  8. Re-issue compromised User JWTs (nsc add user / nsc edit user).
  9. Re-push Account JWTs and distribute .creds via org secret manager.

See docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md for the manual operator outline and A2A_TODO.md for Phase 0 tracker."#
    }
}

impl NscJwtPushTarget for OrgNscAutomationRunbook {
    fn push_uri_docs(&self) -> &'static str {
        r#"nsc push target (operator resolver)

Configure the operator resolver URL before nsc push -a [TENANT_ACCOUNT]:
  - nsc edit operator --account-jwt-server-url <resolver-base>
  - Confirm nsc push resolves [TENANT_ACCOUNT] against the fleet JWT store

Scripted export bundles (ScriptedExportBundle::ORG_DEFAULT) carry account.jwt and user .creds
under accounts/[TENANT_ACCOUNT]/ for org CI to upload after push succeeds.

Full bootstrap context: docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md step 8 (nsc push).
Tracker: A2A_TODO.md — org automation + scripted nsc exports remain future."#
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisioning_stage_labels_are_stable() {
        assert_eq!(
            ProvisioningStage::ScaffoldAccount.label(),
            "scaffold_account"
        );
        assert_eq!(
            ProvisioningStage::WireJetStream.label(),
            "wire_jetstream"
        );
        assert_eq!(ProvisioningStage::RotateUsers.label(), "rotate_users");
    }

    #[test]
    fn export_bundle_layout_is_non_empty() {
        assert!(ScriptedExportBundle::ORG_DEFAULT
            .dir_layout
            .contains("accounts/"));
    }

    #[test]
    fn runbook_docs_link_bootstrap_and_todo() {
        let steps = OrgNscAutomationRunbook.describe_steps();
        assert!(steps.contains("A2A_NSC_ACCOUNT_BOOTSTRAP.md"));
        assert!(steps.contains("A2A_TODO.md"));

        let push = OrgNscAutomationRunbook.push_uri_docs();
        assert!(push.contains("nsc push"));
        assert!(push.contains("A2A_NSC_ACCOUNT_BOOTSTRAP.md"));
    }
}
