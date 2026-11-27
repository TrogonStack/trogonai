use super::*;

#[test]
fn the_last_segment_names_the_lease() {
    let lease = extract_cancel_lease_id("sandbox.provider.hyperlight.cancel.01JTEST0000000000000000000");
    assert_eq!(lease, LeaseId::new("01JTEST0000000000000000000"));
}

#[test]
fn a_subject_without_dots_is_the_lease() {
    assert_eq!(extract_cancel_lease_id("lease"), LeaseId::new("lease"));
}
