use super::*;

fn event(bytes: &[u8]) -> CanonicalEventBytes {
    CanonicalEventBytes::from_json_slice(bytes).expect("valid json fixture")
}

#[test]
fn extend_is_deterministic() {
    let genesis = ChainHash::genesis();
    let event = event(br#"{"a":1}"#);
    let a = extend(&genesis, &event);
    let b = extend(&genesis, &event);
    assert_eq!(a, b);
}

#[test]
fn extend_differs_for_different_events() {
    let genesis = ChainHash::genesis();
    let a = extend(&genesis, &event(br#"{"a":1}"#));
    let b = extend(&genesis, &event(br#"{"a":2}"#));
    assert_ne!(a, b);
}

#[test]
fn extend_differs_for_different_previous_hash() {
    let event = event(br#"{"a":1}"#);
    let a = extend(&ChainHash::genesis(), &event);
    let b = extend(&ChainHash::digest(b"other"), &event);
    assert_ne!(a, b);
}

#[test]
fn extend_chain_produces_distinct_links_per_position() {
    let genesis = ChainHash::genesis();
    let event_a = event(br#"{"a":1}"#);
    let event_b = event(br#"{"a":2}"#);

    let link1 = extend(&genesis, &event_a);
    let link2 = extend(&link1, &event_b);

    // Re-running the same two events in the same order reproduces both links.
    let replay1 = extend(&genesis, &event_a);
    let replay2 = extend(&replay1, &event_b);

    assert_eq!(link1, replay1);
    assert_eq!(link2, replay2);
    assert_ne!(link1, link2);
}
