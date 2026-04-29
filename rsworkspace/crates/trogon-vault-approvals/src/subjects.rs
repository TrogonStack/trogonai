//! NATS subject helpers for the approval workflow.
//!
//! Stream VAULT_PROPOSALS captures create/approve/reject subjects.
//! Status queries are plain core-NATS request-reply (not captured by the stream).

pub const PROPOSALS_STREAM: &str = "VAULT_PROPOSALS";

/// Subjects captured by the VAULT_PROPOSALS stream — passed to stream config.
///
/// **Security invariant**: `vault.proposals.*.approve` and `vault.proposals.*.reject`
/// are intentionally excluded. Approve messages carry plaintext API keys that must
/// never be persisted to JetStream storage. Those subjects are consumed via ephemeral
/// core-NATS subscriptions (at-most-once) so the plaintext lives only in the transient
/// message and is immediately consumed and discarded.
pub fn stream_subjects() -> Vec<String> {
    vec![
        "vault.proposals.*.create".to_string(),
        "vault.proposals.*.state.*".to_string(),
    ]
}

/// Agent publishes a new proposal here.
pub fn create(vault_name: &str) -> String {
    format!("vault.proposals.{vault_name}.create")
}

/// Human approves a pending proposal here.
pub fn approve(vault_name: &str) -> String {
    format!("vault.proposals.{vault_name}.approve")
}

/// Human rejects a pending proposal here.
pub fn reject(vault_name: &str) -> String {
    format!("vault.proposals.{vault_name}.reject")
}

/// Agent queries proposal status via request-reply on this subject.
pub fn status(vault_name: &str, proposal_id: &str) -> String {
    format!("vault.proposals.{vault_name}.status.{proposal_id}")
}

/// Wildcard subscription for all status queries on a vault.
pub fn status_wildcard(vault_name: &str) -> String {
    format!("vault.proposals.{vault_name}.status.>")
}

/// State update published by the service after approve or reject.
///
/// Goes to the VAULT_PROPOSALS stream for audit and external consumers.
/// The service does not subscribe to these — they are fire-and-forget records.
pub fn state_update(vault_name: &str, proposal_id: &str) -> String {
    format!("vault.proposals.{vault_name}.state.{proposal_id}")
}

/// Consumer filter for all proposal events (create + approve + reject) on one vault.
pub fn vault_wildcard(vault_name: &str) -> String {
    format!("vault.proposals.{vault_name}.>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_subject() {
        assert_eq!(create("prod"), "vault.proposals.prod.create");
    }

    #[test]
    fn approve_subject() {
        assert_eq!(approve("prod"), "vault.proposals.prod.approve");
    }

    #[test]
    fn reject_subject() {
        assert_eq!(reject("staging"), "vault.proposals.staging.reject");
    }

    #[test]
    fn status_subject() {
        assert_eq!(
            status("prod", "prop_abc123"),
            "vault.proposals.prod.status.prop_abc123"
        );
    }

    #[test]
    fn state_update_subject() {
        assert_eq!(
            state_update("prod", "prop_abc123"),
            "vault.proposals.prod.state.prop_abc123"
        );
    }

    #[test]
    fn stream_subjects_includes_state_and_excludes_status() {
        let subjects = stream_subjects();
        assert!(subjects.iter().any(|s| s.contains("state")), "stream must capture state updates");
        for s in &subjects {
            assert!(!s.contains("status"), "stream must not capture status: {s}");
        }
    }
}
