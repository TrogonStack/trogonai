//! The audit records of a release decision, built from the facts the release
//! core hands its audit port.

use trogon_audit::{
    Actor, AuditName, CredentialFacts, CredentialTerminal, DecisionFacts, DecisionOutcome, ReasonCode, Record,
    RecordScope, Stamp, Surface,
};
use trogon_credential_aggregate::credential_project_token;
use trogon_credential_domain::CredentialKey;

use super::{DenialRecord, ReleaseAuditRecordError, ReleaseRecord, ReleaseRequest};

/// The must-audit credential use of a release, written before the value is
/// sealed.
pub(crate) fn release_record(record: &ReleaseRecord, stamp: Stamp) -> Result<Record, ReleaseAuditRecordError> {
    let request = record.request();
    let credential = credential_name(request.credential())?;
    // A release serves no connection binding: the credential's own release
    // policy binds it to the runtime service, destination and location, so the
    // credential's name is the binding.
    let mut facts = CredentialFacts::new(CredentialTerminal::Release, credential.clone(), credential);
    facts.destination = Some(name("destination", request.destination().to_string())?);
    facts.location = Some(name("location", request.location().to_string())?);
    Ok(Record::credential_use(scope(request)?, facts, stamp))
}

/// A denial, recorded as a deny decision that stands whether or not its write
/// is acknowledged.
pub(crate) fn denial_record(record: &DenialRecord, stamp: Stamp) -> Result<Record, ReleaseAuditRecordError> {
    let request = record.request();
    let target = credential_name(request.credential())?;
    let mut facts = DecisionFacts::new(Surface::SecretsRelease, target, DecisionOutcome::Deny);
    facts.reason = Some(ReasonCode::new(record.reason().code()).map_err(ReleaseAuditRecordError::Reason)?);
    Ok(Record::decision(scope(request)?, facts, stamp))
}

/// The tenant account and the credential's project scope the record, the
/// runtime service is the caller, and the correlation id ties the record to the
/// use the runtime reports later.
fn scope(request: &ReleaseRequest) -> Result<RecordScope, ReleaseAuditRecordError> {
    let tenant = name("tenant", request.tenant().as_str())?;
    let project =
        credential_project_token(request.credential().owner_id()).map_err(ReleaseAuditRecordError::Project)?;
    let caller = Actor::Service {
        service_id: name("runtime service", request.runtime_service().as_str())?,
    };
    let request_id = name("correlation id", request.correlation_id().as_str())?;
    Ok(RecordScope::new(tenant, project, caller).with_request_id(request_id))
}

/// The credential's resource name (ADR#0046), so the record names the same
/// credential the management API does.
fn credential_name(key: &CredentialKey) -> Result<AuditName, ReleaseAuditRecordError> {
    name(
        "credential",
        format!("projects/{}/credentials/{}", key.owner_id(), key.credential_id()),
    )
}

fn name(field: &'static str, value: impl AsRef<str>) -> Result<AuditName, ReleaseAuditRecordError> {
    AuditName::new(value.as_ref()).map_err(|source| ReleaseAuditRecordError::Name { field, source })
}

#[cfg(test)]
mod tests;
