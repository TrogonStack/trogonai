use std::num::NonZeroU64;

use bytes::Bytes;

use super::require_original_evidence;
use crate::{RegistrationArchiveError, RegistrationArtifact, RegistrationArtifactInput, RegistrationArtifactKind};

fn artifact(kind: RegistrationArtifactKind, bytes: &[u8]) -> Result<RegistrationArtifact, RegistrationArchiveError> {
    RegistrationArtifact::try_from(RegistrationArtifactInput {
        kind,
        bytes: Bytes::copy_from_slice(bytes),
    })
}

#[test]
fn exact_original_evidence_accepts_independently_constructed_artifacts_and_matching_revision()
-> Result<(), RegistrationArchiveError> {
    let expected = artifact(RegistrationArtifactKind::OriginalApproval, b"original")?;
    let observed = artifact(RegistrationArtifactKind::OriginalApproval, b"original")?;
    let revision = NonZeroU64::MIN;
    assert!(require_original_evidence(&expected, &observed, revision, Some(revision)).is_ok());
    assert!(require_original_evidence(&expected, &observed, revision, None).is_ok());
    Ok(())
}

#[test]
fn original_evidence_rejects_truncated_changed_or_extended_original_bytes() -> Result<(), RegistrationArchiveError> {
    let expected = artifact(RegistrationArtifactKind::OriginalApproval, b"original")?;
    let revision = NonZeroU64::MIN;
    for bytes in [b"origina".as_slice(), b"originaL", b"original\0"] {
        let observed = artifact(RegistrationArtifactKind::OriginalApproval, bytes)?;
        for acknowledged in [None, Some(revision)] {
            assert!(matches!(
                require_original_evidence(&expected, &observed, revision, acknowledged),
                Err(RegistrationArchiveError::Integrity)
            ));
        }
    }
    Ok(())
}

#[test]
fn original_evidence_rejects_a_different_kind_even_when_bytes_and_digest_match() -> Result<(), RegistrationArchiveError>
{
    let expected = artifact(RegistrationArtifactKind::OriginalApproval, b"original")?;
    let observed = artifact(RegistrationArtifactKind::BootstrapManifest, b"original")?;
    let revision = NonZeroU64::MIN;
    assert_eq!(expected.bytes(), observed.bytes());
    assert_eq!(expected.digest(), observed.digest());
    assert!(matches!(
        require_original_evidence(&expected, &observed, revision, Some(revision)),
        Err(RegistrationArchiveError::Integrity)
    ));
    Ok(())
}

#[test]
fn readback_evidence_requires_the_actual_ack_revision_while_observation_does_not_invent_one()
-> Result<(), RegistrationArchiveError> {
    let expected = artifact(RegistrationArtifactKind::OriginalApproval, b"original")?;
    let observed = artifact(RegistrationArtifactKind::OriginalApproval, b"original")?;
    let acknowledged = NonZeroU64::MIN;
    let observed_revision = NonZeroU64::MAX;
    assert!(matches!(
        require_original_evidence(&expected, &observed, observed_revision, Some(acknowledged)),
        Err(RegistrationArchiveError::Integrity)
    ));
    assert!(require_original_evidence(&expected, &observed, observed_revision, None).is_ok());
    Ok(())
}
