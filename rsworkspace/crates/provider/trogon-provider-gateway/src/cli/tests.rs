use super::Cli;
use clap::Parser;
use std::net::{IpAddr, Ipv6Addr};
use std::path::Path;

#[test]
fn serves_without_a_subcommand_or_implicit_overrides() {
    let cli = Cli::try_parse_from(["trogon-provider-gateway"]).unwrap();

    assert_eq!(cli.config, None);
    assert_eq!(cli.host, None);
    assert_eq!(cli.port, None);
}

#[test]
fn parses_config_and_typed_address_overrides() {
    let cli = Cli::try_parse_from([
        "trogon-provider-gateway",
        "--config",
        "gateway.toml",
        "--host",
        "::1",
        "--port",
        "0",
    ])
    .unwrap();

    assert_eq!(cli.config.as_deref(), Some(Path::new("gateway.toml")));
    assert_eq!(cli.host, Some(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    assert_eq!(cli.port, Some(0));
}

#[test]
fn rejects_invalid_hosts_and_ports() {
    for arguments in [
        ["trogon-provider-gateway", "--host", "localhost"],
        ["trogon-provider-gateway", "--port", "65536"],
        ["trogon-provider-gateway", "--port", "-1"],
    ] {
        assert!(Cli::try_parse_from(arguments).is_err());
    }
}

#[test]
fn rejects_unknown_arguments() {
    assert!(Cli::try_parse_from(["trogon-provider-gateway", "--nats-url", "localhost:4222"]).is_err());
}
