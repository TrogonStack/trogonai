#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;

#[test]
fn capabilities_header_round_trips_through_parse() {
    let caps = vec![Capability::Interaction, Capability::Clarification, Capability::Payment];
    let (name, value) = capabilities_header(&caps);
    assert_eq!(name, headers::CAPABILITIES);
    let parsed = Capabilities::parse(&value);
    assert_eq!(parsed.0, caps);
}

#[test]
fn aauth_capabilities_lands_on_built_request() {
    let client = reqwest::Client::new();
    let caps = vec![Capability::Interaction, Capability::Payment];
    let request = client
        .get("https://resource.example/thing")
        .aauth_capabilities(&caps)
        .build()
        .expect("request builds");
    let header_value = request
        .headers()
        .get(headers::CAPABILITIES)
        .expect("capabilities header present")
        .to_str()
        .expect("ascii header value");
    assert_eq!(header_value, "interaction, payment");
}
