use super::*;

#[test]
fn accepts_valid_https_url() {
    let url = SourceUrl::parse("https://registry.example.com").unwrap();
    assert_eq!(url.as_str(), "https://registry.example.com");
}

#[test]
fn rejects_non_url_string() {
    assert!(matches!(
        SourceUrl::parse("not a url"),
        Err(SourceUrlError::InvalidUrl(_))
    ));
}
