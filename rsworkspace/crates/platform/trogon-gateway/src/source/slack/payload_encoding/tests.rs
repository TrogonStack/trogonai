use super::*;

#[test]
fn each_encoding_names_its_media_type() {
    assert_eq!(PayloadEncoding::Json.content_type(), "application/json");
    assert_eq!(
        PayloadEncoding::Form.content_type(),
        "application/x-www-form-urlencoded"
    );
}

#[test]
fn a_declared_media_type_matches_regardless_of_case_and_parameters() {
    for header in [
        "application/x-www-form-urlencoded",
        "Application/X-WWW-Form-Urlencoded",
        "application/x-www-form-urlencoded; charset=UTF-8",
        "APPLICATION/X-WWW-FORM-URLENCODED;charset=utf-8",
        " application/x-www-form-urlencoded ;charset=utf-8",
    ] {
        assert!(PayloadEncoding::Form.is_declared_by(header), "{header}");
    }
}

#[test]
fn a_different_or_merely_prefixed_media_type_does_not_match() {
    for header in [
        "application/json",
        "application/x-www-form-urlencoded-extra",
        "text/application/x-www-form-urlencoded",
        "",
    ] {
        assert!(!PayloadEncoding::Form.is_declared_by(header), "{header}");
    }
    assert!(PayloadEncoding::Json.is_declared_by("Application/JSON; charset=utf-8"));
}
