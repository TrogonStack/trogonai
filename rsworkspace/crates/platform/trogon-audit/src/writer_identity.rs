use trogon_nats::NatsToken;

use crate::actor::Actor;
use crate::audit_name::AuditName;
use crate::record_scope::RecordScope;
use crate::writer_id::WriterId;

/// Who a writer is to its chain: the writer id the chain is keyed by, the
/// component token its integrity records are published under, and the tenant
/// and project those records are scoped to.
///
/// The writer's own records, such as the chain reset that opens every chain,
/// are not anyone's action, so their actor is unattributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriterIdentity {
    writer: WriterId,
    component: NatsToken,
    tenant: AuditName,
    project: NatsToken,
}

impl WriterIdentity {
    pub fn new(writer: WriterId, component: NatsToken, tenant: AuditName, project: NatsToken) -> Self {
        Self {
            writer,
            component,
            tenant,
            project,
        }
    }

    pub fn writer(&self) -> &WriterId {
        &self.writer
    }

    pub fn component(&self) -> &NatsToken {
        &self.component
    }

    pub fn tenant(&self) -> &AuditName {
        &self.tenant
    }

    pub fn project(&self) -> &NatsToken {
        &self.project
    }

    pub(crate) fn scope(&self) -> RecordScope {
        RecordScope::new(self.tenant.clone(), self.project.clone(), Actor::Unattributed)
    }
}

#[cfg(test)]
mod tests;
