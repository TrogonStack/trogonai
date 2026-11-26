use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    DirectoryMarker, FsPathEntry, FsPolicy, FsPolicyError, FsPolicyList, GlobPattern, GlobPatternError, LiteralPath,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn rejects<T: DeserializeOwned>(value: serde_json::Value) -> TestResult {
    let from_value = error_of(serde_json::from_value::<T>(value.clone()))?;
    assert_eq!(from_value.classify(), Data);
    let from_str = error_of(serde_json::from_str::<T>(&value.to_string()))?;
    assert_eq!(from_str.classify(), Data);
    Ok(())
}

fn literal(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::Unmarked,
    })
}

fn glob(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(raw)?,
    })
}

fn policy(lists: [Vec<FsPathEntry>; 6]) -> Result<FsPolicy, FsPolicyError> {
    let [a, b, c, d, e, f] = lists;
    FsPolicy::new(a, b, c, d, e, f)
}

fn empty_lists() -> [Vec<FsPathEntry>; 6] {
    [vec![], vec![], vec![], vec![], vec![], vec![]]
}

#[test]
fn a_glob_pattern_collapses_interior_spellings_and_keeps_what_is_semantic() -> TestResult {
    for (raw, want) in [
        ("/abs//x/*", "/abs/x/*"),
        ("/abs/./x/*.pem", "/abs/x/*.pem"),
        ("//**/x", "/**/x"),
        ("/abs/x/*/", "/abs/x/*/"),
        ("/abs/**/*.pem", "/abs/**/*.pem"),
        ("/abs/sub/../*.pem", "/abs/*.pem"),
        ("/abs/x/*/.", "/abs/x/*"),
    ] {
        let pattern = GlobPattern::new(raw)?;
        assert_eq!(pattern.as_str(), want, "{raw}");
        assert_eq!(pattern.to_string(), want, "{raw}");
        assert_eq!(pattern.anchor(), None);
    }
    Ok(())
}

#[test]
fn a_glob_pattern_reads_every_wildcard_form() -> TestResult {
    for raw in [
        "/d/*.ts",
        "/d/**",
        "/d/file?.txt",
        "/d/file[0-9].txt",
        "/d/[!a]",
        "/d/[^a]",
        "/d/[-a]",
        "/d/[a-]",
        "/d/[a-a]",
        "/d/[a[b]",
        "/d/[z-a*]",
        "/d/[z-a?]",
        "/d/[z-a][b]",
        "/d/[\u{1f600}]",
        "/d/[*]",
    ] {
        GlobPattern::new(raw)?;
    }
    Ok(())
}

#[test]
fn a_glob_pattern_rejects_what_the_grammar_forbids() -> TestResult {
    let long = format!("/{}*", "a".repeat(GlobPattern::MAX_LEN));
    let cases: Vec<(String, GlobPatternError)> = vec![
        (String::new(), GlobPatternError::Empty),
        ("*/x".into(), GlobPatternError::NotAbsolute("*/x".into())),
        ("build/*".into(), GlobPatternError::NotAbsolute("build/*".into())),
        ("~/.ssh/*".into(), GlobPatternError::NotAbsolute("~/.ssh/*".into())),
        ("/a/*\0".into(), GlobPatternError::ContainsNul),
        (
            long,
            GlobPatternError::TooLong {
                max: GlobPattern::MAX_LEN,
            },
        ),
        ("/../*.pem".into(), GlobPatternError::EscapesRoot("/../*.pem".into())),
        (
            "/abs/*/../x*".into(),
            GlobPatternError::AmbiguousParent("/abs/*/../x*".into()),
        ),
        (
            "/abs/[a-c]/../x".into(),
            GlobPatternError::AmbiguousParent("/abs/[a-c]/../x".into()),
        ),
        (
            "/data/secrets".into(),
            GlobPatternError::NotAGlob("/data/secrets".into()),
        ),
        (
            "/data/[unclosed".into(),
            GlobPatternError::NotAGlob("/data/[unclosed".into()),
        ),
        ("/data/[]".into(), GlobPatternError::NotAGlob("/data/[]".into())),
        ("/data/[z-a]".into(), GlobPatternError::NotAGlob("/data/[z-a]".into())),
        ("/data/[]a]".into(), GlobPatternError::NotAGlob("/data/[]a]".into())),
        ("/data/[[z-a]".into(), GlobPatternError::NotAGlob("/data/[[z-a]".into())),
        ("/".into(), GlobPatternError::NotAGlob("/".into())),
        (
            "/d/[\u{1f600}-\u{1f601}]".into(),
            GlobPatternError::NotAGlob("/d/[\u{1f600}-\u{1f601}]".into()),
        ),
    ];
    for (raw, want) in cases {
        assert_eq!(error_of(GlobPattern::new(raw.clone()))?, want, "{raw:?}");
        assert_eq!(error_of(raw.parse::<GlobPattern>())?, want, "{raw:?}");
        assert_eq!(error_of(GlobPattern::try_from(raw.clone()))?, want, "{raw:?}");
        rejects::<GlobPattern>(json!(raw))?;
    }
    Ok(())
}

#[test]
fn a_glob_pattern_error_describes_itself() {
    for (error, text) in [
        (GlobPatternError::Empty, "pattern cannot be empty"),
        (
            GlobPatternError::NotAbsolute("x".into()),
            "pattern \"x\" must be absolute",
        ),
        (GlobPatternError::ContainsNul, "pattern contains a NUL byte"),
        (
            GlobPatternError::TooLong { max: 4096 },
            "pattern is longer than 4096 bytes",
        ),
        (
            GlobPatternError::EscapesRoot("/..".into()),
            "pattern \"/..\" climbs above the root with \"..\"",
        ),
        (
            GlobPatternError::AmbiguousParent("/a/*/../b".into()),
            "pattern \"/a/*/../b\" has a \"..\" that follows a wildcard segment and cannot be folded",
        ),
        (
            GlobPatternError::NotAGlob("/a".into()),
            "\"/a\" has no wildcard; declare it as a literal path",
        ),
    ] {
        assert_eq!(error.to_string(), text);
    }
}

#[test]
fn a_glob_pattern_knows_when_it_ends_in_a_separator() -> TestResult {
    assert!(GlobPattern::new("/data/*/")?.ends_with_separator());
    assert!(!GlobPattern::new("/data/*")?.ends_with_separator());
    Ok(())
}

#[test]
fn an_anchored_pattern_keeps_its_anchor_as_a_name() -> TestResult {
    let anchor = LiteralPath::new("/w/a[b]")?;
    let from_str = GlobPattern::anchored(anchor.clone(), "/*.pem")?;
    let from_string = GlobPattern::anchored(anchor.clone(), String::from("/*.pem"))?;
    assert_eq!(from_str, from_string);
    assert_eq!(from_str.as_str(), "/w/a[b]/*.pem");
    assert_eq!(from_str.to_string(), "/w/a[b]/*.pem");
    assert_eq!(from_str.anchor(), Some(&anchor));
    let at_root = GlobPattern::anchored(LiteralPath::new("/")?, "/x//*.pem")?;
    assert_eq!(at_root.as_str(), "/x/*.pem");
    let trailing = GlobPattern::anchored(anchor, "/*/")?;
    assert_eq!(trailing.as_str(), "/w/a[b]/*/");
    assert!(trailing.ends_with_separator());
    Ok(())
}

#[test]
fn an_anchored_pattern_needs_a_wildcard_tail_that_starts_with_a_separator() -> TestResult {
    let anchor = LiteralPath::new("/w")?;
    for (tail, want) in [
        ("*.pem", GlobPatternError::NotAbsolute("*.pem".into())),
        ("", GlobPatternError::NotAbsolute(String::new())),
        ("/plain", GlobPatternError::NotAGlob("/plain".into())),
        ("/a\0*", GlobPatternError::ContainsNul),
        ("/../*", GlobPatternError::EscapesRoot("/../*".into())),
    ] {
        assert_eq!(error_of(GlobPattern::anchored(anchor.clone(), tail))?, want, "{tail:?}");
    }
    Ok(())
}

#[test]
fn a_glob_pattern_serializes_plain_as_a_string_and_anchored_as_an_object() -> TestResult {
    let plain = GlobPattern::new("/w/*.pem")?;
    assert_eq!(serde_json::to_value(&plain)?, json!("/w/*.pem"));
    assert_eq!(serde_json::from_value::<GlobPattern>(json!("/w/*.pem"))?, plain);
    assert_eq!(
        serde_json::from_value::<GlobPattern>(json!("/w//x/*.pem"))?.as_str(),
        "/w/x/*.pem"
    );

    let anchored = GlobPattern::anchored(LiteralPath::new("/w/a[b]")?, "/**/*.pem")?;
    let wire = json!({ "anchor": "/w/a[b]", "tail": "/**/*.pem" });
    assert_eq!(serde_json::to_value(&anchored)?, wire);
    assert_eq!(serde_json::from_value::<GlobPattern>(wire)?, anchored);

    for bad in [
        json!({ "anchor": "/w", "tail": "*.pem" }),
        json!({ "anchor": "/w", "tail": "/plain" }),
        json!({ "anchor": "rel", "tail": "/*" }),
        json!({ "anchor": "/w" }),
        json!(7),
    ] {
        rejects::<GlobPattern>(bad)?;
    }
    Ok(())
}

#[test]
fn an_entry_distinguishes_a_literal_from_a_glob_on_the_wire() -> TestResult {
    let from_literal: FsPathEntry = serde_json::from_value(json!({ "kind": "literal", "path": "/a/*" }))?;
    let from_glob: FsPathEntry = serde_json::from_value(json!({ "kind": "glob", "pattern": "/a/*" }))?;
    assert_eq!(from_literal, literal("/a/*")?);
    assert_eq!(from_glob, glob("/a/*")?);
    assert_ne!(from_literal, from_glob);
    assert_eq!(
        serde_json::to_value(&from_literal)?,
        json!({ "kind": "literal", "path": "/a/*" })
    );
    assert_eq!(
        serde_json::to_value(&from_glob)?,
        json!({ "kind": "glob", "pattern": "/a/*" })
    );
    Ok(())
}

#[test]
fn a_literal_entry_carries_the_directory_marker_as_a_boolean() -> TestResult {
    let marked: FsPathEntry = serde_json::from_value(json!({ "kind": "literal", "path": "/a", "directory": true }))?;
    assert_eq!(
        marked,
        FsPathEntry::Literal {
            path: LiteralPath::new("/a")?,
            directory: DirectoryMarker::Directory,
        }
    );
    assert_eq!(
        serde_json::to_value(&marked)?,
        json!({ "kind": "literal", "path": "/a", "directory": true })
    );
    let unmarked: FsPathEntry = serde_json::from_value(json!({ "kind": "literal", "path": "/a", "directory": false }))?;
    assert_eq!(unmarked, literal("/a")?);
    assert_eq!(
        serde_json::to_value(&unmarked)?,
        json!({ "kind": "literal", "path": "/a" })
    );
    assert!(DirectoryMarker::Directory.is_directory());
    assert!(!DirectoryMarker::Directory.is_unmarked());
    assert!(!DirectoryMarker::Unmarked.is_directory());
    assert!(DirectoryMarker::Unmarked.is_unmarked());
    assert_eq!(DirectoryMarker::default(), DirectoryMarker::Unmarked);
    assert!(bool::from(DirectoryMarker::Directory));
    assert!(!bool::from(DirectoryMarker::Unmarked));
    Ok(())
}

#[test]
fn an_entry_canonicalizes_a_literal_path_and_rejects_bad_ones() -> TestResult {
    let entry: FsPathEntry = serde_json::from_value(json!({ "kind": "literal", "path": "/a//b/./c/" }))?;
    assert_eq!(entry, literal("/a/b/c")?);
    for bad in [
        json!({ "kind": "literal", "path": "" }),
        json!({ "kind": "literal", "path": "rel" }),
        json!({ "kind": "literal", "path": "/../x" }),
        json!({ "kind": "glob", "pattern": "/plain" }),
        json!({ "kind": "glob", "path": "/a/*" }),
        json!({ "path": "/a" }),
        json!({ "kind": "other", "path": "/a" }),
        json!({ "kind": "literal", "path": "/a", "extra": 1 }),
        json!({ "kind": "literal", "path": "/a", "directory": "yes" }),
    ] {
        rejects::<FsPathEntry>(bad)?;
    }
    Ok(())
}

#[test]
fn a_policy_round_trips_and_an_empty_one_is_an_empty_object() -> TestResult {
    let built = FsPolicy::new(
        vec![literal("/home")?],
        vec![literal("/home/.netrc")?],
        vec![glob("/home/*/project")?],
        vec![literal("/home/ca.pem")?],
        vec![literal("/work")?],
        vec![glob("/work/**/*.pem")?],
    )?;
    let wire = json!({
        "deny_read": [{ "kind": "literal", "path": "/home" }],
        "credential_deny_read": [{ "kind": "literal", "path": "/home/.netrc" }],
        "allow_read_within_deny": [{ "kind": "glob", "pattern": "/home/*/project" }],
        "own_allow_within_deny": [{ "kind": "literal", "path": "/home/ca.pem" }],
        "allow_write": [{ "kind": "literal", "path": "/work" }],
        "deny_write": [{ "kind": "glob", "pattern": "/work/**/*.pem" }],
    });
    assert_eq!(serde_json::to_value(&built)?, wire);
    assert_eq!(serde_json::from_value::<FsPolicy>(wire)?, built);
    assert_eq!(built.deny_read(), [literal("/home")?]);
    assert_eq!(built.credential_deny_read(), [literal("/home/.netrc")?]);
    assert_eq!(built.allow_read_within_deny(), [glob("/home/*/project")?]);
    assert_eq!(built.own_allow_within_deny(), [literal("/home/ca.pem")?]);
    assert_eq!(built.allow_write(), [literal("/work")?]);
    assert_eq!(built.deny_write(), [glob("/work/**/*.pem")?]);
    assert!(!built.is_empty());

    assert_eq!(serde_json::to_value(FsPolicy::default())?, json!({}));
    assert_eq!(serde_json::from_value::<FsPolicy>(json!({}))?, FsPolicy::default());
    assert!(FsPolicy::default().is_empty());
    assert_eq!(policy(empty_lists())?, FsPolicy::default());
    Ok(())
}

#[test]
fn a_policy_refuses_unknown_fields_and_bad_entries() -> TestResult {
    rejects::<FsPolicy>(json!({ "deny_reed": [] }))?;
    rejects::<FsPolicy>(json!({ "deny_read": [{ "kind": "glob", "pattern": "/plain" }] }))?;
    rejects::<FsPolicy>(json!({ "deny_read": "/home" }))?;
    Ok(())
}

#[test]
fn a_policy_refuses_an_oversized_list_in_any_position() -> TestResult {
    let lists = [
        FsPolicyList::DenyRead,
        FsPolicyList::CredentialDenyRead,
        FsPolicyList::AllowReadWithinDeny,
        FsPolicyList::OwnAllowWithinDeny,
        FsPolicyList::AllowWrite,
        FsPolicyList::DenyWrite,
    ];
    for (position, list) in lists.into_iter().enumerate() {
        let mut oversized = empty_lists();
        oversized[position] = vec![literal("/w")?; FsPolicy::MAX_ENTRIES_PER_LIST + 1];
        assert_eq!(
            error_of(policy(oversized))?,
            FsPolicyError::TooManyEntries {
                list,
                max: FsPolicy::MAX_ENTRIES_PER_LIST,
                got: FsPolicy::MAX_ENTRIES_PER_LIST + 1,
            }
        );
        let mut full = empty_lists();
        full[position] = vec![literal("/w")?; FsPolicy::MAX_ENTRIES_PER_LIST];
        policy(full)?;
    }
    let entries = vec![json!({ "kind": "literal", "path": "/w" }); FsPolicy::MAX_ENTRIES_PER_LIST + 1];
    rejects::<FsPolicy>(json!({ "allow_write": entries }))?;
    Ok(())
}

#[test]
fn a_policy_refuses_an_inert_deny_glob() -> TestResult {
    for spelling in ["/data/*/", "/data/**/", "/data/?/", "/data/[ab]/"] {
        let suggestion = spelling.trim_end_matches('/');
        for (position, list) in [
            (0, FsPolicyList::DenyRead),
            (1, FsPolicyList::CredentialDenyRead),
            (5, FsPolicyList::DenyWrite),
        ] {
            let mut lists = empty_lists();
            lists[position] = vec![glob(spelling)?];
            assert_eq!(
                error_of(policy(lists))?,
                FsPolicyError::InertDenyGlob {
                    list,
                    pattern: spelling.to_string(),
                    suggestion: suggestion.to_string(),
                }
            );
        }
    }
    rejects::<FsPolicy>(json!({ "deny_write": [{ "kind": "glob", "pattern": "/data/*/" }] }))?;
    Ok(())
}

#[test]
fn a_policy_accepts_slash_free_deny_globs_and_trailing_slash_elsewhere() -> TestResult {
    policy([
        vec![glob("/data/*")?, glob("/data/**")?, literal("/data/secrets/")?],
        vec![],
        vec![glob("/data/*/")?],
        vec![glob("/data/*/")?],
        vec![glob("/work/*/")?],
        vec![glob("/data/*")?, literal("/data/secrets/")?],
    ])?;
    Ok(())
}

#[test]
fn a_policy_error_describes_itself() {
    assert_eq!(
        FsPolicyError::TooManyEntries {
            list: FsPolicyList::AllowWrite,
            max: 1024,
            got: 1025,
        }
        .to_string(),
        "fs_policy.allow_write supports at most 1024 entries, got 1025"
    );
    assert_eq!(
        FsPolicyError::InertDenyGlob {
            list: FsPolicyList::DenyWrite,
            pattern: "/data/*/".into(),
            suggestion: "/data/*".into(),
        }
        .to_string(),
        "fs_policy.deny_write glob \"/data/*/\" ends in a separator, so as a pattern it can match no path and would \
         deny nothing. Write \"/data/*\", or add a \"**\" segment to match at any depth"
    );
}

#[test]
fn every_list_is_named_as_on_the_wire() {
    for (list, name) in [
        (FsPolicyList::DenyRead, "deny_read"),
        (FsPolicyList::CredentialDenyRead, "credential_deny_read"),
        (FsPolicyList::AllowReadWithinDeny, "allow_read_within_deny"),
        (FsPolicyList::OwnAllowWithinDeny, "own_allow_within_deny"),
        (FsPolicyList::AllowWrite, "allow_write"),
        (FsPolicyList::DenyWrite, "deny_write"),
    ] {
        assert_eq!(list.to_string(), name);
    }
}
