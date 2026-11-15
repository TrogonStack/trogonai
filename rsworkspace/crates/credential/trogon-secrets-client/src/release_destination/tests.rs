use std::num::NonZeroU16;

use trogon_credential_domain::ReleaseHost;

use super::ReleaseDestination;

fn port(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value).unwrap()
}

#[test]
fn the_same_host_on_another_port_is_another_destination() {
    let host = ReleaseHost::new("api.github.com").unwrap();

    assert_ne!(
        ReleaseDestination::new(&host, port(443)),
        ReleaseDestination::new(&host, port(8443))
    );
}

#[test]
fn keeps_the_normalized_host_and_the_port() {
    let destination = ReleaseDestination::new(&ReleaseHost::new("API.GitHub.com").unwrap(), port(8443));

    assert_eq!(destination.host(), "api.github.com");
    assert_eq!(destination.port(), port(8443));
}

#[test]
fn names_the_same_host_and_port_on_the_wire() {
    let destination = ReleaseDestination::new(&ReleaseHost::new("api.github.com").unwrap(), port(8443));

    let wire = destination.to_wire();

    assert_eq!(wire.host, "api.github.com");
    assert_eq!(wire.port, 8443);
}
