use super::{ConfigError, HOST_ENV, PORT_ENV, load};
use crate::cli::Cli;
use clap::Parser;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use trogon_std::env::InMemoryEnv;

fn cli(arguments: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("trogon-provider-gateway").chain(arguments.iter().copied())).unwrap()
}

fn config_file(content: &str) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    std::fs::write(file.path(), content).unwrap();
    file
}

#[test]
fn defaults_to_loopback_and_port_8317() {
    let config = load(&cli(&[]), &InMemoryEnv::new()).unwrap();

    assert_eq!(config.bind_addr(), SocketAddr::from((Ipv4Addr::LOCALHOST, 8317)));
}

#[test]
fn toml_overrides_defaults() {
    let file = config_file("host = '::1'\nport = 0\n");
    let config = load(&cli(&["--config", file.path().to_str().unwrap()]), &InMemoryEnv::new()).unwrap();

    assert_eq!(config.bind_addr(), SocketAddr::from((Ipv6Addr::LOCALHOST, 0)));
}

#[test]
fn environment_overrides_toml() {
    let file = config_file("host = '127.0.0.2'\nport = 1234\n");
    let env = InMemoryEnv::new();
    env.set(HOST_ENV, "::1");
    env.set(PORT_ENV, "4321");
    let config = load(&cli(&["--config", file.path().to_str().unwrap()]), &env).unwrap();

    assert_eq!(config.bind_addr(), SocketAddr::from((Ipv6Addr::LOCALHOST, 4321)));
}

#[test]
fn cli_overrides_environment_and_toml_per_field() {
    let file = config_file("host = '127.0.0.2'\nport = 1234\n");
    let env = InMemoryEnv::new();
    env.set(HOST_ENV, "::1");
    env.set(PORT_ENV, "4321");
    let config = load(
        &cli(&["--config", file.path().to_str().unwrap(), "--host", "127.0.0.3"]),
        &env,
    )
    .unwrap();

    assert_eq!(config.bind_addr(), SocketAddr::from(([127, 0, 0, 3], 4321)));

    let config = load(&cli(&["--config", file.path().to_str().unwrap(), "--port", "0"]), &env).unwrap();

    assert_eq!(config.bind_addr(), SocketAddr::from((Ipv6Addr::LOCALHOST, 0)));
}

#[test]
fn validates_only_effective_values_after_overrides() {
    let file = config_file("host = 'invalid'\nport = -1\n");
    let env = InMemoryEnv::new();
    env.set(HOST_ENV, "127.0.0.1");
    env.set(PORT_ENV, "8318");
    let config = load(&cli(&["--config", file.path().to_str().unwrap()]), &env).unwrap();
    assert_eq!(config.bind_addr(), SocketAddr::from((Ipv4Addr::LOCALHOST, 8318)));

    env.set(HOST_ENV, "invalid");
    env.set(PORT_ENV, "65536");
    let config = load(
        &cli(&[
            "--config",
            file.path().to_str().unwrap(),
            "--host",
            "::1",
            "--port",
            "0",
        ]),
        &env,
    )
    .unwrap();
    assert_eq!(config.bind_addr(), SocketAddr::from((Ipv6Addr::LOCALHOST, 0)));
}

#[test]
fn rejects_invalid_effective_environment_values() {
    let env = InMemoryEnv::new();
    env.set(HOST_ENV, "localhost");
    assert!(matches!(load(&cli(&[]), &env), Err(ConfigError::Host(_))));

    env.set(HOST_ENV, "127.0.0.1");
    for port in ["-1", "65536", "abc", ""] {
        env.set(PORT_ENV, port);
        assert!(matches!(load(&cli(&[]), &env), Err(ConfigError::Port(_))));
    }
}

#[test]
fn rejects_invalid_effective_file_values() {
    let env = InMemoryEnv::new();
    let file = config_file("host = 'localhost'\n");
    assert!(matches!(
        load(&cli(&["--config", file.path().to_str().unwrap()]), &env),
        Err(ConfigError::Host(_))
    ));

    for port in [-1, 65536] {
        let file = config_file(&format!("port = {port}\n"));
        assert!(matches!(
            load(&cli(&["--config", file.path().to_str().unwrap()]), &env),
            Err(ConfigError::FilePort(_))
        ));
    }
}

#[test]
fn rejects_unknown_toml_keys_and_malformed_toml() {
    for content in ["porrt = 8317\n", "[nats]\nurl = 'localhost:4222'\n", "host = ["] {
        let file = config_file(content);
        assert!(matches!(
            load(&cli(&["--config", file.path().to_str().unwrap()]), &InMemoryEnv::new()),
            Err(ConfigError::ParseFile { .. })
        ));
    }
}

#[test]
fn explicit_missing_file_surfaces_io_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing.toml");

    assert!(matches!(
        load(&cli(&["--config", path.to_str().unwrap()]), &InMemoryEnv::new()),
        Err(ConfigError::ReadFile { source, .. }) if source.kind() == std::io::ErrorKind::NotFound
    ));
}

#[test]
fn ignores_unrelated_environment_values() {
    let env = InMemoryEnv::new();
    env.set("NATS_URL", "invalid");
    env.set("TROGON_PROVIDER_GATEWAY_UNKNOWN", "invalid");
    let config = load(&cli(&[]), &env).unwrap();

    assert_eq!(config.bind_addr().ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
}
