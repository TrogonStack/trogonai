use super::*;

fn fresh_xkey_public() -> String {
    XKey::new().public_key()
}

#[test]
fn parse_round_trips_through_as_str_and_display() {
    let raw = fresh_xkey_public();
    let xp = XkeyPublic::parse(raw.clone()).unwrap();
    assert_eq!(xp.as_str(), raw);
    assert_eq!(xp.to_string(), raw);
    assert!(format!("{xp:?}").starts_with("XkeyPublic"));
}

#[test]
fn parse_trims_surrounding_whitespace() {
    let raw = fresh_xkey_public();
    let padded = format!("  {raw}\n");
    assert_eq!(XkeyPublic::parse(padded).unwrap().as_str(), raw);
}

#[test]
fn parse_rejects_empty_input() {
    let e = XkeyPublic::parse("   ").unwrap_err();
    assert!(matches!(e, AuthCalloutError::WireFormat(_)));
}

#[test]
fn parse_rejects_non_xkey_value() {
    let e = XkeyPublic::parse("not-a-key").unwrap_err();
    assert!(matches!(e, AuthCalloutError::WireFormat(_)));
}

#[test]
fn to_xkey_yields_usable_keypair() {
    let raw = fresh_xkey_public();
    let xp = XkeyPublic::parse(raw).unwrap();
    xp.to_xkey().unwrap();
}
