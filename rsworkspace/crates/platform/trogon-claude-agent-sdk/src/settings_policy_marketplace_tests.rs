#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests build marketplace sources as the JSON a managed document holds."
    )
)]

use serde_json::json;

use super::*;

#[test]
fn collapses_dot_segments_like_the_pinned_resolver() {
    assert_eq!(collapse_segments("/a/./b/../c//d/"), "a/c/d");
    assert_eq!(collapse_segments("../x"), "x");
}

#[test]
fn strips_trailing_slashes_and_git_suffixes_until_none_is_left() {
    assert_eq!(strip_dot_git("a/b.git/.git//"), "a/b");
    assert_eq!(strip_dot_git("a/b"), "a/b");
}

#[test]
fn decodes_escapes_like_decode_uri_component() {
    assert_eq!(decode_uri_component("a%2Ab").as_deref(), Some("a*b"));
    assert_eq!(decode_uri_component("%E0%A4%A"), None);
    assert_eq!(decode_uri_component("%FF"), None);
}

#[test]
fn flags_the_sources_that_can_never_match() {
    let cases = [
        (json!({"source": "hostPattern", "hostPattern": "("}), true),
        (json!({"source": "pathPattern", "pathPattern": "^/srv/.*"}), false),
        (json!({"source": "github", "repo": "owner/*"}), false),
        (json!({"source": "github", "repo": "-owner/*"}), true),
        (
            json!({"source": "git", "url": "ssh://git@ssh.github.com:443/owner/*"}),
            true,
        ),
        (json!({"source": "git", "url": "git@example.com:owner/*"}), false),
        (
            json!({"source": "git", "url": "https://github.com/owner/repo", "ref": "main*"}),
            true,
        ),
        (json!({"source": "npm", "package": "x"}), false),
    ];
    for (entry, expected) in cases {
        assert_eq!(unenforceable(&entry), expected, "{entry}");
    }
}
