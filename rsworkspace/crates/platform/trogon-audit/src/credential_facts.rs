use crate::audit_name::AuditName;
use crate::credential_terminal::CredentialTerminal;
use crate::record_id::RecordId;

/// A credential release or injection, by resource name and binding id. The value
/// never appears, nor a placeholder, nor a digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialFacts {
    pub kind: CredentialTerminal,
    pub credential: AuditName,
    pub binding_id: AuditName,
    pub destination: Option<AuditName>,
    /// The decision record an injection served.
    pub decision_record_id: Option<RecordId>,
    /// Where in the outbound request the credential is placed.
    pub location: Option<AuditName>,
}

impl CredentialFacts {
    pub fn new(kind: CredentialTerminal, credential: AuditName, binding_id: AuditName) -> Self {
        Self {
            kind,
            credential,
            binding_id,
            destination: None,
            decision_record_id: None,
            location: None,
        }
    }
}
