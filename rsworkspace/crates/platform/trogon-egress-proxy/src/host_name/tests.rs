use super::*;

#[test]
fn folds_to_lowercase() {
    assert_eq!(
        HostName::new("API.Example.COM").expect("valid").as_str(),
        "api.example.com"
    );
}

#[test]
fn accepts_single_label_and_underscore_names() {
    for raw in [
        "localhost",
        "db",
        "_dmarc.example.org",
        "xn--bcher-kva.example",
        "a-b.c",
    ] {
        assert!(HostName::new(raw).is_ok(), "{raw:?} should be accepted");
    }
}

#[test]
fn rejects_patterns_paths_and_whitespace() {
    for raw in [
        "*.example.com",
        "*",
        "example.com/path",
        "a b.com",
        "a\tb.com",
        "user@example.com",
        "a:b",
    ] {
        assert!(
            matches!(HostName::new(raw), Err(HostNameError::InvalidCharacter { .. })),
            "{raw:?} must be rejected"
        );
    }
}

#[test]
fn rejects_non_ascii() {
    assert!(matches!(
        HostName::new("bücher.example"),
        Err(HostNameError::InvalidCharacter { .. })
    ));
}

#[test]
fn rejects_empty_labels_and_trailing_dot() {
    assert_eq!(HostName::new(""), Err(HostNameError::Empty));
    assert_eq!(HostName::new("a..b"), Err(HostNameError::EmptyLabel));
    assert_eq!(HostName::new(".a.b"), Err(HostNameError::EmptyLabel));
    assert_eq!(HostName::new("a.b."), Err(HostNameError::EmptyLabel));
}

#[test]
fn rejects_length_overruns() {
    assert_eq!(
        HostName::new(format!("{}.com", "a".repeat(64))),
        Err(HostNameError::LabelTooLong { max: 63 })
    );
    let long = format!("{}.com", vec!["a".repeat(60); 5].join("."));
    assert_eq!(HostName::new(long), Err(HostNameError::TooLong { max: 253 }));
}

#[test]
fn rejects_hyphen_at_label_edge() {
    assert_eq!(HostName::new("-a.com"), Err(HostNameError::HyphenAtLabelEdge));
    assert_eq!(HostName::new("a-.com"), Err(HostNameError::HyphenAtLabelEdge));
}

#[test]
fn rejects_names_a_resolver_would_read_as_an_address() {
    for raw in [
        "127.1",
        "2130706433",
        "0x7f.1",
        "0x7f000001",
        "1.2.3",
        "10.0.0.1",
        "1e3",
    ] {
        assert_eq!(
            HostName::new(raw),
            Err(HostNameError::LooksLikeAddress),
            "{raw:?} must be rejected"
        );
    }
}
