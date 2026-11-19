use super::parse_children;

const PRODUCTION_LIMIT: usize = 4096;

fn identifiers(count: usize) -> Vec<u8> {
    (1..=count)
        .map(|identity| format!("{identity} "))
        .collect::<String>()
        .into_bytes()
}

#[test]
#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn child_parser_preserves_below_and_exact_production_limit() {
    for count in [PRODUCTION_LIMIT - 1, PRODUCTION_LIMIT] {
        let snapshot = parse_children(&identifiers(count), false, PRODUCTION_LIMIT)
            .unwrap_or_else(|_| panic!("complete positive kernel IDs"));
        assert_eq!(snapshot.children.len(), count);
        assert_eq!(snapshot.children.first(), Some(&1));
        assert_eq!(snapshot.children.last(), Some(&(count as i32)));
        assert!(!snapshot.has_more);
    }
}

#[test]
#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn child_parser_retains_bounded_prefix_above_production_limit() {
    let snapshot = parse_children(&identifiers(PRODUCTION_LIMIT + 1), false, PRODUCTION_LIMIT)
        .unwrap_or_else(|_| panic!("complete bounded prefix"));
    assert_eq!(snapshot.children.len(), PRODUCTION_LIMIT);
    assert_eq!(snapshot.children.last(), Some(&(PRODUCTION_LIMIT as i32)));
    assert!(snapshot.has_more);
}

#[test]
#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn child_parser_never_authorizes_a_truncated_pid_token() {
    let mut bytes = identifiers(PRODUCTION_LIMIT - 1);
    bytes.extend_from_slice(b"12345");
    for truncated in [false, true] {
        let snapshot = parse_children(&bytes, truncated, PRODUCTION_LIMIT)
            .unwrap_or_else(|_| panic!("complete prefix survives incomplete tail"));
        assert_eq!(snapshot.children.len(), PRODUCTION_LIMIT - 1);
        assert_eq!(snapshot.children.last(), Some(&((PRODUCTION_LIMIT - 1) as i32)));
        assert!(!snapshot.children.contains(&12345));
        assert!(snapshot.has_more);
    }
    let snapshot =
        parse_children(b"7", true, PRODUCTION_LIMIT).unwrap_or_else(|_| panic!("incomplete first token is uncertain"));
    assert!(snapshot.children.is_empty());
    assert!(snapshot.has_more);
}

#[test]
#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn child_parser_keeps_exhausted_whitespace_boundary_uncertain() {
    let snapshot =
        parse_children(b"71 72 ", true, PRODUCTION_LIMIT).unwrap_or_else(|_| panic!("complete tokens at byte bound"));
    assert_eq!(snapshot.children, [71, 72]);
    assert!(snapshot.has_more);
    let snapshot =
        parse_children(b"", false, PRODUCTION_LIMIT).unwrap_or_else(|_| panic!("complete empty kernel listing"));
    assert!(snapshot.children.is_empty());
    assert!(!snapshot.has_more);
}

#[test]
fn child_parser_refuses_malformed_and_nonpositive_complete_ids() {
    for bytes in [
        b"0 ".as_slice(),
        b"-1 ",
        b"+1 ",
        b"2147483648 ",
        b"1e3 ",
        b"1\0 ",
        b"a ",
        b"1 \xff ",
    ] {
        assert!(parse_children(bytes, false, PRODUCTION_LIMIT).is_err());
    }
}
