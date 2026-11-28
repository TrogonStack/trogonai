use super::*;

#[test]
fn a_subject_that_names_a_lease_is_cancelled() {
    let id = lease_to_cancel("sandbox.provider.azure_container_apps.cancel.lease-1");
    assert!(matches!(id, Ok(id) if id == LeaseId::new("lease-1")));
}

#[test]
fn a_subject_without_a_lease_segment_is_an_error_not_a_lease() {
    assert!(matches!(
        lease_to_cancel("sandbox.cancel."),
        Err(AcaError::CancelSubject(subject)) if subject == "sandbox.cancel."
    ));
    assert!(matches!(lease_to_cancel(""), Err(AcaError::CancelSubject(_))));
}
