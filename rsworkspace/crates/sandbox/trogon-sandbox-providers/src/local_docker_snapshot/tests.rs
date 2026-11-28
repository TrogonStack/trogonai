use super::split_image_ref;

#[test]
fn a_tag_is_split_off_and_a_missing_one_is_latest() {
    assert_eq!(split_image_ref("repo:tag"), ("repo", "tag"));
    assert_eq!(split_image_ref("repo"), ("repo", "latest"));
    assert_eq!(split_image_ref("org/repo:v1"), ("org/repo", "v1"));
}

#[test]
fn a_registry_port_is_not_taken_for_a_tag() {
    assert_eq!(
        split_image_ref("localhost:5000/repo"),
        ("localhost:5000/repo", "latest")
    );
    assert_eq!(
        split_image_ref("localhost:5000/org/repo:v2"),
        ("localhost:5000/org/repo", "v2")
    );
}
