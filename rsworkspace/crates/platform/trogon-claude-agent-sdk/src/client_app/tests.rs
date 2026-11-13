use super::{format_client_app, normalize_client_app};

#[test]
fn accepts_scoped_and_versioned_labels() {
    for label in [
        "app1",
        "my-app/1.2.3",
        "@scope/pkg",
        "@scope/pkg/1.0.0+x",
        "a.b_c-d/1!2+3",
    ] {
        assert_eq!(normalize_client_app(label), Some(label), "{label}");
    }
}

#[test]
fn rejects_paths_spaces_and_oversized_labels() {
    for label in [
        "",
        "/abs/path",
        "a b",
        "-lead",
        "a/b/c/d",
        "@/x",
        "name/",
        "@scope",
        &"a".repeat(129),
    ] {
        assert_eq!(normalize_client_app(label), None, "{label}");
    }
    assert!(normalize_client_app(&"a".repeat(128)).is_some());
}

#[test]
fn format_strips_build_metadata_and_skips_generic_names() {
    assert_eq!(
        format_client_app("tool", Some("1.0.0+local")).as_deref(),
        Some("tool/1.0.0")
    );
    assert_eq!(format_client_app("tool", Some("bad version")).as_deref(), Some("tool"));
    assert_eq!(format_client_app("Main", Some("1.0.0")), None);
    assert_eq!(format_client_app("tool", None).as_deref(), Some("tool"));
}
