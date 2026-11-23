use trogon_sandbox_contract::{OciImageRef, pattern_matches_legacy, pattern_matches_oci_image_ref};

#[test]
fn legacy_pattern_matches_exact_or_single_trailing_wildcard() {
    assert!(!pattern_matches_legacy("", ""));
    assert!(!pattern_matches_legacy("", "ubuntu"));
    assert!(pattern_matches_legacy("ubuntu:24.04", "ubuntu:24.04"));
    assert!(!pattern_matches_legacy("ubuntu:24.04", "ubuntu:22.04"));
    assert!(pattern_matches_legacy("*", ""));
    assert!(pattern_matches_legacy("*", "anything"));
    assert!(pattern_matches_legacy(
        "registry.example.com/team-a/*",
        "registry.example.com/team-a/img:tag"
    ));
    assert!(!pattern_matches_legacy(
        "registry.example.com/team-a/*",
        "registry.example.com/team-ab:tag"
    ));
    assert!(pattern_matches_legacy("team-*/app*", "team-*/app*"));
    assert!(!pattern_matches_legacy("team-*/app*", "team-a/app"));
}

#[test]
fn image_pattern_matches_through_the_allowlist_string() {
    let prefixed = OciImageRef::parse_unqualified("registry.example.com/team-a/foo:tag");
    assert!(pattern_matches_oci_image_ref(
        "registry.example.com/team-a/*",
        &prefixed
    ));

    let ubuntu = OciImageRef::parse_unqualified("ubuntu:24.04");
    assert!(pattern_matches_oci_image_ref("ubuntu:24.04", &ubuntu));
    assert!(!pattern_matches_oci_image_ref("debian:12", &ubuntu));
    assert!(!pattern_matches_oci_image_ref("", &ubuntu));
}

#[test]
fn image_pattern_matches_by_part_when_the_allowlist_string_differs() {
    let tagged = OciImageRef::parse_unqualified("ghcr.io/team-a/foo:1.0");
    let untagged = OciImageRef::parse_unqualified("ghcr.io/team-a/foo");

    assert!(pattern_matches_oci_image_ref("ghcr.io/team-a/foo", &tagged));
    assert!(pattern_matches_oci_image_ref("ghcr.io/team-a/*:*", &tagged));
    assert!(pattern_matches_oci_image_ref("ghcr.io/team-a/*:*", &untagged));
    assert!(pattern_matches_oci_image_ref("team-a/*:*", &tagged));
    assert!(pattern_matches_oci_image_ref("ghcr.io/team-a/f*:1.0", &tagged));

    assert!(!pattern_matches_oci_image_ref("docker.io/team-a/foo", &tagged));
    assert!(!pattern_matches_oci_image_ref("ghcr.io/team-b/*", &tagged));
    assert!(!pattern_matches_oci_image_ref("ghcr.io/team-a/foo:2.0", &tagged));
    assert!(!pattern_matches_oci_image_ref("ghcr.io/team-a/foo:1.0", &untagged));
    assert!(!pattern_matches_oci_image_ref("ghcr.io/te*m-a/*:*", &tagged));
}
