use serde_json::Value;

use super::*;
use crate::bridge_git::parse_repository;

const URL_GOLDENS: &str = include_str!("../tests/fixtures/bridge_create_session/urls.json");

#[test]
fn parses_addresses_like_the_upstream_bridge() {
    let cases: Vec<Value> = serde_json::from_str(URL_GOLDENS).unwrap_or_default();
    assert!(cases.len() > 500);
    for case in &cases {
        let url = case["url"].as_str().unwrap_or_default();
        let parsed = parse_repository(url).map(|repo| {
            (
                format!("https://{}/{}/{}", repo.host, repo.owner, repo.name),
                format!("{}/{}", repo.owner, repo.name),
            )
        });
        let expected = case["source"]
            .as_str()
            .map(|source| (source.to_owned(), case["repo"].as_str().unwrap_or_default().to_owned()));
        assert_eq!(parsed, expected, "{url:?}");
    }
}

#[test]
fn refuses_bracket_forms() {
    assert!(rejected("ssh://[::1]x/acme/widgets"));
    assert!(rejected("git@[::1]x:acme/widgets"));
    assert!(rejected("https://host/[x]"));
    assert!(rejected("[::1]/acme/widgets"));
    assert!(rejected("file://@[host]/x"));
    assert!(!rejected("ssh://git@[::1]:22/acme/widgets"));
    assert!(!rejected("git@[::1]:acme/widgets"));
    assert!(!rejected("ssh://[::1]/acme/widgets"));
    assert!(!rejected("file:///srv/repos/widgets"));
}

#[test]
fn refuses_nul_and_backslash_authorities() {
    assert!(rejected("https://github.com%00.evil.com/acme/widgets"));
    assert!(rejected("https://github.com\0/acme/widgets"));
    assert!(rejected("https://github.com\\@evil.com/acme/widgets"));
    assert!(rejected("ssh://git@github.com\\x/acme/widgets"));
    assert!(!rejected("https://github.com/acme/wid\\gets"));
    assert!(!rejected("github.com/acme/widgets"));
    assert!(!backslash_authority("acme/widgets"));
    assert!(!backslash_authority("ssh://git@github.com/acme/wid\\gets"));
}

#[test]
fn refuses_hosts_that_are_read_differently() {
    assert!(rejected("ssh://git@github.com%40evil.com/acme/widgets"));
    assert!(!rejected("https://GitHub.com./acme/widgets"));
    assert!(!rejected("https://www.github.com/acme/widgets"));
    assert!(!rejected("git@github.com:acme/widgets.git"));
}

#[test]
fn decodes_escape_runs_the_way_the_upstream_helpers_do() {
    assert_eq!(decode_latin1("a%41%00%zz%4"), "aA%00%zz%4");
    assert_eq!(decode_latin1("%c3%a9"), "\u{c3}\u{a9}");
    assert_eq!(decode_uri_runs("%c3%a9x%00%41"), "\u{e9}x%00A");
    assert_eq!(decode_uri_runs("%ff%41"), "\u{ff}A");
    assert_eq!(decode_uri_runs("plain"), "plain");
}

#[test]
fn reads_hosts_out_of_addresses() {
    assert_eq!(hostname("https://Example.COM/x").as_deref(), Some("example.com"));
    assert_eq!(hostname("not a url"), None);
    assert_eq!(hostname("file:///x"), None);
    assert_eq!(git_host("file://host/x", false), None);
    assert_eq!(git_host("  https://A.com/x", false).as_deref(), Some("a.com"));
    assert_eq!(git_host("ssh://u@h.com:22/x", false).as_deref(), Some("h.com"));
    assert_eq!(git_host("u@h.com:x/y", false).as_deref(), Some("h.com"));
    assert_eq!(git_host("u@h%2ecom:x/y", true).as_deref(), Some("h.com"));
    assert_eq!(git_host("a/b:c", false), None);
    assert_eq!(git_host("plain", false), None);
    assert_eq!(git_host("ssh://[::1]:22/x", false).as_deref(), Some("[::1]"));
    assert_eq!(decoded_url_host("ssh://h%2ecom/x").as_deref(), Some("h.com"));
    assert_eq!(decoded_url_host("https://h.com/x").as_deref(), Some("h.com"));
}

#[test]
fn splits_hosts_around_brackets_and_ports() {
    assert_eq!(
        split_address("git@[::1]:x/y", ':'),
        Some(("[::1]".to_owned(), "x/y".to_owned()))
    );
    assert_eq!(
        split_address("[::1]:22/x", '/'),
        Some(("[::1]".to_owned(), "/x".to_owned()))
    );
    assert_eq!(
        split_address("u@[::1]:22/x", '/'),
        Some(("[::1]".to_owned(), "/x".to_owned()))
    );
    assert_eq!(
        split_address("u@[::1]:x/y", ':'),
        Some(("[::1]".to_owned(), "x/y".to_owned()))
    );
    assert_eq!(
        split_address("u@[::1]:99999/y", '/'),
        Some(("[::1]".to_owned(), "/y".to_owned()))
    );
    assert_eq!(
        split_address("h.com:22/x", '/'),
        Some(("h.com".to_owned(), "/x".to_owned()))
    );
    assert_eq!(
        split_address("h.com:ab/x", '/'),
        Some(("h.com:ab".to_owned(), "/x".to_owned()))
    );
    assert_eq!(split_address("/x:y", ':'), None);
    assert_eq!(split_address("@/x", '/'), None);
}

#[test]
fn drops_only_a_numeric_port_in_range() {
    assert_eq!(without_port("h:22"), "h");
    assert_eq!(without_port("h: 22"), "h");
    assert_eq!(without_port("h:+22"), "h");
    assert_eq!(without_port("h:-0"), "h");
    assert_eq!(without_port("h:-5"), "h:-5");
    assert_eq!(without_port("h:65536"), "h:65536");
    assert_eq!(without_port("h:0000022"), "h");
    assert_eq!(without_port("h:123456"), "h:123456");
    assert_eq!(without_port("h:"), "h:");
    assert_eq!(without_port("h:x"), "h:x");
    assert_eq!(without_port("h"), "h");
}

#[test]
fn folds_hosts_to_one_spelling() {
    assert_eq!(fold_host("GitHub.COM."), "github.com");
    assert_eq!(fold_host("git\thub.com"), "github.com");
    assert_eq!(fold_host(""), "");
    assert_eq!(fold_host("a b"), "a b");
    assert_eq!(fold_host("a:80"), "a:80");
    assert_eq!(fold_host("a/b"), "a/b");
    assert_eq!(fold_host("B\u{fc}cher.de"), "xn--bcher-kva.de");
    assert_eq!(canonical_host("WWW.WWW.GitHub.com"), "github.com");
    assert_eq!(canonical_host("gitlab.com"), "gitlab.com");
    assert!(is_github("www.github.com"));
    assert!(!is_github("github.com.evil.com"));
}

#[test]
fn recognizes_scheme_and_drive_shapes() {
    assert_eq!(split_scheme("ssh://x", false), Some(("ssh", "x")));
    assert_eq!(split_scheme("1ssh://x", false), None);
    assert_eq!(split_scheme("1ssh://x", true), Some(("1ssh", "x")));
    assert_eq!(split_scheme("ssh:/x", false), None);
    assert_eq!(split_scheme("", true), None);
    assert!(drive_letter("C:\\x"));
    assert!(!drive_letter("ab:x"));
    assert!(!drive_letter(""));
    assert_eq!(bracket_at("x[::1]y", 1), Some(("[::1]", 6)));
    assert_eq!(bracket_at("[]", 0), None);
    assert_eq!(bracket_at("[::1", 0), None);
    assert_eq!(bracket_at("ab", 5), None);
    assert_eq!(url_bracket_prefix("u@[::1]:22/x"), Some(("[::1]", 10)));
    assert_eq!(url_bracket_prefix("[::1]x"), None);
    assert_eq!(scp_bracket_prefix("u@[::1]:x"), Some(("[::1]", 7)));
    assert_eq!(scp_bracket_prefix("@[::1]:x"), None);
    assert_eq!(scp_bracket_prefix("u:[::1]:x"), None);
    assert_eq!(scp_bracket_prefix("u@[::1]x"), None);
    assert_eq!(scp_bracket_prefix("ux"), None);
}
