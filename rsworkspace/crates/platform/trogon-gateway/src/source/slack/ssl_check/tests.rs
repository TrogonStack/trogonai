use super::*;

const FORM: Option<&str> = Some("application/x-www-form-urlencoded");

#[test]
fn a_form_post_with_ssl_check_one_is_a_probe() {
    assert_eq!(SslCheck::detect(FORM, b"ssl_check=1"), SslCheck::Requested);
}

#[test]
fn other_fields_do_not_change_the_verdict() {
    assert_eq!(
        SslCheck::detect(FORM, b"token=abc&ssl_check=1&command=%2Ftrogon"),
        SslCheck::Requested
    );
}

#[test]
fn only_the_exact_value_one_counts() {
    for body in [
        "ssl_check=0",
        "ssl_check=",
        "ssl_check=true",
        "ssl_check=11",
        "ssl_check",
        "ssl%5Fcheck=2",
    ] {
        assert_eq!(
            SslCheck::detect(FORM, body.as_bytes()),
            SslCheck::NotRequested,
            "{body}"
        );
    }
}

#[test]
fn a_repeated_key_is_not_a_probe() {
    for body in [
        "ssl_check=1&ssl_check=1",
        "ssl_check=1&ssl_check=2",
        "ssl_check=2&ssl_check=1",
    ] {
        assert_eq!(
            SslCheck::detect(FORM, body.as_bytes()),
            SslCheck::NotRequested,
            "{body}"
        );
    }
}

#[test]
fn the_content_type_must_match_exactly() {
    for content_type in [
        None,
        Some("application/json"),
        Some("application/x-www-form-urlencoded; charset=utf-8"),
        Some("Application/X-WWW-Form-Urlencoded"),
        Some(" application/x-www-form-urlencoded"),
    ] {
        assert_eq!(
            SslCheck::detect(content_type, b"ssl_check=1"),
            SslCheck::NotRequested,
            "{content_type:?}"
        );
    }
}

#[test]
fn an_encoded_key_is_decoded_before_comparing() {
    assert_eq!(SslCheck::detect(FORM, b"ssl%5Fcheck=1"), SslCheck::Requested);
}

#[test]
fn a_body_without_the_key_is_not_a_probe() {
    assert_eq!(SslCheck::detect(FORM, b""), SslCheck::NotRequested);
    assert_eq!(SslCheck::detect(FORM, b"command=%2Ftrogon"), SslCheck::NotRequested);
}
