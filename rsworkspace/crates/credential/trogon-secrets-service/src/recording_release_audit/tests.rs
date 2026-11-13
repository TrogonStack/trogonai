use std::time::UNIX_EPOCH;

use nkeys::XKey;
use trogon_credential_domain::CredentialVersion;

use super::*;
use crate::DenialReason;
use crate::RequesterXkey;
use crate::fixtures::{request, tenant};

fn release() -> ReleaseRecord {
    ReleaseRecord::new(
        request(&tenant()),
        CredentialVersion::new(1).unwrap(),
        RequesterXkey::parse(&XKey::new().public_key()).unwrap(),
        UNIX_EPOCH,
    )
}

fn denial() -> DenialRecord {
    DenialRecord::new(request(&tenant()), DenialReason::NotReleasable)
}

#[tokio::test]
async fn a_refused_release_write_is_not_recorded() {
    let audit = RecordingReleaseAudit::new();
    audit.refuse_releases();

    assert_eq!(
        audit.record_release(&release()).await,
        Err(ReleaseAuditError::new("release record refused"))
    );
    assert!(audit.releases().is_empty());
}

#[tokio::test]
async fn a_refused_denial_write_is_not_recorded() {
    let audit = RecordingReleaseAudit::new();
    audit.refuse_denials();

    assert_eq!(
        audit.record_denial(&denial()).await,
        Err(ReleaseAuditError::new("denial record refused"))
    );
    assert!(audit.denials().is_empty());
}

#[tokio::test]
async fn records_each_acknowledged_write_in_order() {
    let audit = RecordingReleaseAudit::new();
    let release = release();
    let denial = denial();

    audit.record_release(&release).await.unwrap();
    audit.record_denial(&denial).await.unwrap();

    assert_eq!(audit.releases(), vec![release.clone()]);
    assert_eq!(audit.denials(), vec![denial.clone()]);
    assert_eq!(release.version(), CredentialVersion::new(1).unwrap());
    assert_eq!(release.valid_until(), UNIX_EPOCH);
    assert_eq!(release.request().correlation_id().as_str(), "use-1");
    assert!(release.recipient().as_str().starts_with('X'));
    assert_eq!(denial.reason(), &DenialReason::NotReleasable);
    assert_eq!(denial.request().correlation_id().as_str(), "use-1");
}
