use super::*;

#[test]
fn every_material_error_has_its_own_reason_code() {
    let codes: Vec<_> = [
        MaterialError::Bounds,
        MaterialError::RevocationUnsupported,
        MaterialError::Identity,
        MaterialError::Profile,
        MaterialError::Certificate,
        MaterialError::Key,
        MaterialError::Unavailable,
    ]
    .into_iter()
    .map(|error| SourceFault::from(error).code())
    .collect();
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), codes.len());
    assert_eq!(
        codes,
        [
            "bounds_exceeded",
            "crl_bearing_response",
            "identity_mismatch",
            "invalid_svid_profile",
            "invalid_certificate_path",
            "key_mismatch",
            "material_unavailable",
        ]
    );
}

#[test]
fn response_errors_map_to_their_reasons() {
    assert_eq!(
        SourceFault::from(&WorkloadResponseError::BundleConflict).code(),
        "bundle_conflict"
    );
    assert_eq!(
        SourceFault::from(&WorkloadResponseError::Material(MaterialError::Profile)).code(),
        "invalid_svid_profile"
    );
    let svid = spiffe::X509Svid::parse_from_der(&[0], &[0]).unwrap_err();
    assert_eq!(
        SourceFault::from(&WorkloadResponseError::from(svid)).code(),
        "unparsable_response"
    );
}

#[test]
fn transport_reasons_are_stable() {
    let codes: Vec<_> = [
        SourceFault::ConnectFailed,
        SourceFault::ConnectTimedOut,
        SourceFault::FirstResponseTimedOut,
        SourceFault::StreamEnded,
        SourceFault::StreamError,
    ]
    .into_iter()
    .map(SourceFault::code)
    .collect();
    assert_eq!(
        codes,
        [
            "connect_failed",
            "connect_timed_out",
            "first_response_timed_out",
            "stream_ended",
            "stream_error"
        ]
    );
}

#[test]
fn jwt_bundle_errors_map_to_their_reasons() {
    assert_eq!(
        SourceFault::from(JwtBundleError::TooManyBundles).code(),
        "bounds_exceeded"
    );
    assert_eq!(SourceFault::from(JwtBundleError::TooLarge).code(), "bounds_exceeded");
    assert_eq!(
        SourceFault::from(JwtBundleError::NoAuthorities).code(),
        "invalid_jwt_bundle"
    );
}
