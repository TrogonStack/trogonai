use super::*;

#[test]
fn keeps_the_host_and_port() {
    let destination = ReleaseDestination::new(
        ReleaseHost::new("api.github.com").unwrap(),
        NonZeroU16::new(443).unwrap(),
    );

    assert_eq!(destination.host().as_str(), "api.github.com");
    assert_eq!(destination.port().get(), 443);
    assert_eq!(destination.to_string(), "api.github.com:443");
}
