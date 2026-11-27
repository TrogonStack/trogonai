use super::*;

const AMBIGUOUS: &[(&str, &str)] = &[
    ("/a/../b", "/b"),
    ("/a/./b", "/a/b"),
    ("/a/..", "/"),
    ("/a/.", "/a/"),
    ("/a/%2e%2e/b", "/b"),
    ("/a/.%2E/b", "/b"),
    ("/a/%2e/b", "/a/b"),
    ("/a;/../b", "/b"),
    ("/a//../b", "/a/b"),
    ("//k/x", "/k/x"),
    ("///k?q=1", "/k?q=1"),
    ("/a\\b", "/a/b"),
    ("/\\k", "/k"),
    ("//k/../b", "/b"),
    ("//k\\b", "/k/b"),
    ("/\\/k", "/k"),
    ("/a\\..\\b", "/b"),
    ("/a\\%2e%2e\\b", "/b"),
    ("/a\\b/../c", "/a/c"),
    ("/a%5cb/../c", "/c"),
    ("//k%5cb", "/k%5cb"),
    ("/a%23/../b", "/b"),
    ("/a%23b\\c", "/a%23b/c"),
    ("//k%23", "/k%23"),
];

const UNAMBIGUOUS: &[&str] = &[
    "/",
    "/a/b/?x=1",
    "/a//b",
    "/a/b//",
    "/a%2Fb",
    "/a%2f..%2fb",
    "/a/..%2fb",
    "/a/%252e%252e/b",
    "/a/...",
    "/a/..b",
    "/a/.hidden",
    "/a/..;/b",
    "/a;p=1/b;q?x=1",
    "/a?x=../y&z=//&w=\\",
    "/%61%7e",
    "/a%zz",
    "/a%5cb",
    "/a%5c..%5cb",
    "/a/..%5cb",
    "/a%23b",
    "/a/..%23",
    "/a/%2e%2e%23",
    "/a?x=%23",
    "/a?",
    "/a??",
    "/a{b}|c^d\"e<f>g`h",
    "/a?q='x'&r=\"y\"",
];

const FRAGMENT: &[(&str, &str)] = &[
    ("/p#f", "/p"),
    ("/p#", "/p"),
    ("/p#f?q", "/p"),
    ("/p?q#f", "/p?q"),
    ("/p?#f", "/p?"),
    ("/#f", "/"),
    ("/p#f#g", "/p"),
    ("/p%23a#f", "/p%23a"),
    ("/p#/../x", "/p"),
    ("/p#\\x", "/p"),
];

const FRAGMENT_AMBIGUOUS: &[(&str, &str, &str)] = &[
    ("/a/../b#x", "/a/../b", "/b"),
    ("/a/%2e%2e/b#x", "/a/%2e%2e/b", "/b"),
    ("//k#x", "//k", "/k"),
    ("/a\\b#x", "/a\\b", "/a/b"),
    ("/a\\b/../c?q#x", "/a\\b/../c?q", "/a/c?q"),
];

const MODES: [AmbiguousTargets; 3] = [
    AmbiguousTargets::Refuse,
    AmbiguousTargets::AsSpelled,
    AmbiguousTargets::Normalize,
];

fn read(raw: &str, method: &str, mode: AmbiguousTargets) -> Result<RequestTarget, RefusalError> {
    RequestTarget::read(raw, method, TargetMode::Judged(mode))
}

fn both_spellings(raw: &str) -> [String; 2] {
    [raw.to_string(), format!("http://h.test:8080{raw}")]
}

#[test]
fn an_ambiguous_target_is_refused_unless_a_mode_picks_a_form() -> Result<(), Box<dyn std::error::Error>> {
    for (raw, normalized) in AMBIGUOUS {
        for target in both_spellings(raw) {
            let forms = TargetForms {
                spelled: raw.to_string(),
                normalized: normalized.to_string(),
                ambiguous: true,
            };
            assert_eq!(
                read(&target, "GET", AmbiguousTargets::Refuse).err(),
                Some(RefusalError::AmbiguousTarget),
                "{target}"
            );
            let as_spelled = read(&target, "GET", AmbiguousTargets::AsSpelled)?;
            assert_eq!(as_spelled.spelled, *raw, "{target}");
            assert_eq!(as_spelled.forms, forms, "{target}");
            let normalizing = read(&target, "GET", AmbiguousTargets::Normalize)?;
            assert_eq!(normalizing.spelled, *normalized, "{target}");
            assert_eq!(normalizing.forms, forms, "{target}");
        }
    }
    Ok(())
}

#[test]
fn an_unambiguous_target_goes_out_byte_for_byte_in_every_mode() -> Result<(), Box<dyn std::error::Error>> {
    for raw in UNAMBIGUOUS {
        for target in both_spellings(raw) {
            for mode in MODES {
                let read = read(&target, "GET", mode)?;
                assert!(!read.forms.ambiguous, "{target}");
                assert_eq!(read.forms.spelled, *raw, "{target}");
                if mode != AmbiguousTargets::Normalize {
                    assert_eq!(read.spelled, *raw, "{target} {mode:?}");
                } else {
                    assert_eq!(read.spelled, read.forms.normalized, "{target}");
                }
            }
        }
    }
    Ok(())
}

#[test]
fn a_target_is_spelled_in_origin_form_with_non_ascii_percent_encoded() -> Result<(), Box<dyn std::error::Error>> {
    for (raw, spelled) in [
        ("*", "*"),
        ("http://h.test", "/"),
        ("HTTPS://u:p@h.test?x", "/?x"),
        ("http://[::1]:80/a", "/a"),
        ("/caf\u{e9}?q=\u{20ac}", "/caf%C3%A9?q=%E2%82%AC"),
        ("http://caf\u{e9}.test/\u{1f600}", "/%F0%9F%98%80"),
    ] {
        for mode in MODES {
            let read = read(raw, "OPTIONS", mode)?;
            assert_eq!(read.spelled, spelled, "{raw} {mode:?}");
            assert!(!read.forms.ambiguous, "{raw}");
        }
    }
    Ok(())
}

#[test]
fn an_escape_is_not_the_character_it_spells() -> Result<(), Box<dyn std::error::Error>> {
    let read_get = |raw| read(raw, "GET", AmbiguousTargets::Refuse);
    assert_eq!(read_get("/a%7Bb")?.forms.normalized, "/a%7Bb");
    assert!(!read_get("/a%5Cb")?.forms.ambiguous);
    assert_eq!(read_get("/a%5C../b")?.spelled, "/a%5C../b");
    Ok(())
}

#[test]
fn a_character_that_only_looks_like_a_dot_or_a_slash_is_data() -> Result<(), Box<dyn std::error::Error>> {
    let read = read("/a/\u{ff0e}\u{ff0e}/b\u{ff0f}c", "GET", AmbiguousTargets::Refuse)?;
    assert_eq!(read.spelled, "/a/%EF%BC%8E%EF%BC%8E/b%EF%BC%8Fc");
    assert!(!read.forms.ambiguous);
    Ok(())
}

#[test]
fn a_dot_segment_beside_a_character_outside_ascii_is_still_ambiguous() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        read("/\u{e9}/../b", "GET", AmbiguousTargets::Refuse).err(),
        Some(RefusalError::AmbiguousTarget)
    );
    Ok(())
}

#[test]
fn a_fragment_is_in_neither_form_and_never_makes_a_target_ambiguous() -> Result<(), Box<dyn std::error::Error>> {
    for (raw, left) in FRAGMENT {
        for target in both_spellings(raw) {
            for mode in MODES {
                let with = read(&target, "GET", mode)?;
                let without = read(left, "GET", mode)?;
                assert_eq!(with.spelled, without.spelled, "{target} {mode:?}");
                assert_eq!(with.forms, without.forms, "{target}");
            }
        }
        assert_eq!(read(raw, "GET", AmbiguousTargets::Refuse)?.spelled, *left);
    }
    Ok(())
}

#[test]
fn what_is_left_of_a_fragment_is_ambiguous_as_it_would_be_without_one() -> Result<(), Box<dyn std::error::Error>> {
    for (raw, left, normalized) in FRAGMENT_AMBIGUOUS {
        for mode in MODES {
            let with = read(raw, "GET", mode);
            let without = read(left, "GET", mode);
            assert_eq!(
                with.map(|read| (read.spelled, read.forms)),
                without.map(|read| (read.spelled, read.forms)),
                "{raw} {mode:?}"
            );
        }
        let forms = read(raw, "GET", AmbiguousTargets::AsSpelled)?.forms;
        assert!(forms.ambiguous);
        assert_eq!(forms.normalized, *normalized);
    }
    Ok(())
}

#[test]
fn what_a_fragment_holds_is_not_read() -> Result<(), Box<dyn std::error::Error>> {
    let plain = read("/p?q", "GET", AmbiguousTargets::Refuse)?;
    for fragment in ["\u{fffd}", "\u{e9}", " ", "\n#", "\u{1f600}", "\t\u{7f}"] {
        let with = read(&format!("/p?q#{fragment}"), "GET", AmbiguousTargets::Refuse)?;
        assert_eq!(with.spelled, plain.spelled, "{fragment:?}");
        assert_eq!(with.forms, plain.forms, "{fragment:?}");
    }
    Ok(())
}

#[test]
fn a_replacement_character_is_refused_for_what_it_may_stand_in_for() -> Result<(), Box<dyn std::error::Error>> {
    for raw in ["/caf\u{fffd}", "/a?q=\u{fffd}", "/\u{fffd}x", "http://h.test/\u{fffd}"] {
        for mode in MODES {
            assert_eq!(read(raw, "GET", mode).err(), Some(RefusalError::TargetBytes), "{raw:?}");
        }
    }
    assert_eq!(
        RefusalError::TargetBytes.to_string(),
        "malformed request-target: it holds bytes that are not UTF-8, or U+FFFD"
    );
    Ok(())
}

#[test]
fn a_target_of_no_known_shape_has_no_spelling_to_forward() -> Result<(), Box<dyn std::error::Error>> {
    for raw in [
        "*x",
        "h.test/x",
        "http://[::1/x",
        "ftp://h.test/x",
        "http:///h.test/x",
        "http:/h.test/x",
        "http://h.test\\x",
        "http://h.test\\@other.test/x",
        "#x",
        "*#x",
        "/a b",
        "/a\tb",
        "/a\u{7f}",
        "@other.test/x",
        "http://",
        "",
    ] {
        for mode in MODES {
            assert_eq!(
                read(raw, "GET", mode).err(),
                Some(RefusalError::MalformedTarget),
                "{raw:?} {mode:?}"
            );
        }
    }
    assert_eq!(
        read("*", "GET", AmbiguousTargets::Refuse).err(),
        Some(RefusalError::MalformedTarget)
    );
    Ok(())
}

#[test]
fn an_authority_ended_by_a_fragment_leaves_the_root() -> Result<(), Box<dyn std::error::Error>> {
    for raw in [
        "http://h.test#x",
        "http://h.test#x/y",
        "http://h.test#",
        "http://h.test#@other.test/x",
    ] {
        for mode in MODES {
            let read = read(raw, "GET", mode)?;
            assert_eq!(read.spelled, "/", "{raw}");
            assert_eq!(read.authority.as_deref(), Some("h.test"), "{raw}");
            assert!(!read.forms.ambiguous);
        }
    }
    Ok(())
}

#[test]
fn without_a_judge_the_parsed_path_and_query_are_forwarded() -> Result<(), Box<dyn std::error::Error>> {
    for (raw, forwarded) in [
        ("/a/../b", "/b"),
        ("/a/%2e%2e/b", "/b"),
        ("//k/x", "//k/x"),
        ("/a?", "/a"),
        ("/a\\b", "/a/b"),
        ("/a;p=1/b;q?x=1", "/a;p=1/b;q?x=1"),
        ("/a/./b/%2e%2e/c", "/a/c"),
    ] {
        for target in both_spellings(raw) {
            let read = RequestTarget::read(&target, "GET", TargetMode::ParsedUrl)?;
            assert_eq!(read.spelled, forwarded, "{target}");
        }
    }
    Ok(())
}

#[test]
fn a_target_keeps_its_authority_and_scheme() -> Result<(), Box<dyn std::error::Error>> {
    let read = read("https://h.test:8443/a", "GET", AmbiguousTargets::Refuse)?;
    assert!(read.secure);
    assert_eq!(read.authority.as_deref(), Some("h.test:8443"));
    Ok(())
}

#[tokio::test]
async fn a_request_trailer_is_read_and_never_relayed() -> Result<(), Box<dyn std::error::Error>> {
    let mut client: &[u8] = b"5\r\nhello\r\n0\r\nX-Trail: value\r\nX-Other: two\r\n\r\n";
    let mut buffered = Vec::new();
    let mut upstream = Vec::new();
    let sent = relay_request_body(&mut client, &mut buffered, &mut upstream, Framing::Chunked).await?;
    assert_eq!(upstream, b"5\r\nhello\r\n0\r\n\r\n");
    assert_eq!(sent, upstream.len() as u64);
    assert!(buffered.is_empty());
    Ok(())
}
