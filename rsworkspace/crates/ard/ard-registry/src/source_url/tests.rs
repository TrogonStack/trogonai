use super::*;

#[test]
fn accepts_valid_https_url() {
    let url = SourceUrl::parse("https://registry.example.com").unwrap();
    assert_eq!(url.as_str(), "https://registry.example.com");
}

#[test]
fn accepts_http_localhost() {
    let url = SourceUrl::parse("http://127.0.0.1").unwrap();
    assert_eq!(url.as_str(), "http://127.0.0.1");
}

#[test]
fn rejects_non_url_string() {
    assert!(matches!(
        SourceUrl::parse("not a url"),
        Err(SourceUrlError::InvalidUrl(_))
    ));
}

#[test]
fn rejects_ftp_scheme() {
    assert!(matches!(
        SourceUrl::parse("ftp://example.com"),
        Err(SourceUrlError::UnsupportedScheme(_))
    ));
}

#[test]
fn rejects_url_with_path() {
    assert!(matches!(
        SourceUrl::parse("https://example.com/path"),
        Err(SourceUrlError::NotAnOrigin)
    ));
}

#[test]
fn rejects_url_with_query() {
    assert!(matches!(
        SourceUrl::parse("https://example.com/?q=1"),
        Err(SourceUrlError::NotAnOrigin)
    ));
}

#[test]
fn rejects_url_with_fragment() {
    assert!(matches!(
        SourceUrl::parse("https://example.com/#f"),
        Err(SourceUrlError::NotAnOrigin)
    ));
}

#[test]
fn rejects_url_with_credentials() {
    assert!(matches!(
        SourceUrl::parse("https://user:pass@example.com"),
        Err(SourceUrlError::CredentialsNotAllowed)
    ));
    assert!(matches!(
        SourceUrl::parse("https://user@example.com"),
        Err(SourceUrlError::CredentialsNotAllowed)
    ));
}
