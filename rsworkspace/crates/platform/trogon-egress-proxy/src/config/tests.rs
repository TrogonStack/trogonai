use trogon_nats::NatsTls;
use trogon_std::env::InMemoryEnv;

use super::*;

const REQUIRED: [&str; 19] = [
    "trogon-egress-proxy",
    "--policy-file",
    "policy.toml",
    "--tls-cert",
    "server.pem",
    "--tls-key",
    "server.key",
    "--client-ca",
    "ca.pem",
    "--trust-domain",
    "trogon.example",
    "--tenant",
    "acme",
    "--project",
    "payments",
    "--instance",
    "egress-1",
    "--nats-url",
    "nats://audit.example:4222",
];

fn resolve(args: Args) -> Result<Config, ConfigError> {
    Config::resolve(args, &InMemoryEnv::new())
}

fn parse(extra: &[&str]) -> Result<Args, clap::Error> {
    Args::try_parse_from(REQUIRED.iter().chain(extra.iter()))
}

#[test]
fn defaults_are_conservative() {
    let config = resolve(parse(&[]).expect("args")).expect("config");
    assert_eq!(config.listen, "127.0.0.1:3128".parse().expect("addr"));
    assert!(config.non_tls_ports.is_empty());
    assert_eq!(config.limits, Limits::default());
    assert_eq!(config.trust_domain.as_str(), "trogon.example");
}

#[test]
fn the_policy_file_is_required() {
    let without_policy = [
        "trogon-egress-proxy",
        "--tls-cert",
        "a",
        "--tls-key",
        "b",
        "--client-ca",
        "c",
        "--trust-domain",
        "d",
        "--tenant",
        "t",
        "--project",
        "p",
        "--instance",
        "i",
        "--nats-url",
        "nats://n:4222",
    ];
    assert!(Args::try_parse_from(without_policy).is_err());
}

#[test]
fn every_audit_option_is_required() {
    for option in ["--tenant", "--project", "--instance", "--nats-url"] {
        let at = REQUIRED.iter().position(|arg| *arg == option).expect("option");
        let mut without: Vec<&str> = REQUIRED.to_vec();
        without.drain(at..at + 2);
        assert!(Args::try_parse_from(without).is_err(), "{option} must be required");
    }
}

#[test]
fn the_audit_options_become_typed_audit_config() {
    let config = resolve(parse(&[]).expect("args")).expect("config");
    assert_eq!(config.audit.tenant.as_str(), "acme");
    assert_eq!(config.audit.project.as_str(), "payments");
    assert_eq!(config.audit.writer.as_str(), "egress-proxy:egress-1");
    assert_eq!(config.audit.nats.servers, vec!["nats://audit.example:4222".to_owned()]);
}

#[test]
fn nats_urls_are_a_comma_separated_list() {
    let mut args = parse(&[]).expect("args");
    args.nats_url = "nats://a:4222, nats://b:4222,".to_owned();
    let config = resolve(args).expect("config");
    assert_eq!(config.audit.nats.servers, ["nats://a:4222", "nats://b:4222"]);
}

#[test]
fn nats_tls_is_required_unless_the_environment_disables_it() {
    let secure = resolve(parse(&[]).expect("args")).expect("config");
    assert_eq!(secure.audit.nats.tls, Ok(NatsTls::default()));

    let env = InMemoryEnv::new();
    env.set("NATS_TLS_DISABLED", "true");
    let plain = Config::resolve(parse(&[]).expect("args"), &env).expect("config");
    assert_eq!(plain.audit.nats.tls, Ok(NatsTls::Disabled));
}

#[test]
fn a_blank_nats_url_is_a_typed_error() {
    let mut args = parse(&[]).expect("args");
    args.nats_url = " , ".to_owned();
    assert_eq!(resolve(args).expect_err("no servers"), ConfigError::NoNatsServers);
}

#[test]
fn invalid_audit_names_are_typed_errors() {
    let mut args = parse(&[]).expect("args");
    args.tenant = "has space".to_owned();
    assert!(matches!(resolve(args).expect_err("tenant"), ConfigError::Tenant(_)));

    let mut args = parse(&[]).expect("args");
    args.project = "has.dot".to_owned();
    assert!(matches!(resolve(args).expect_err("project"), ConfigError::Project(_)));

    let mut args = parse(&[]).expect("args");
    args.instance = "bad instance".to_owned();
    assert!(matches!(resolve(args).expect_err("instance"), ConfigError::Instance(_)));
}

#[test]
fn the_process_environment_is_the_default_source() {
    let config = Config::try_from(parse(&[]).expect("args")).expect("config");
    assert_eq!(config.audit.tenant.as_str(), "acme");
}

#[test]
fn non_tls_ports_are_a_comma_separated_list() {
    let config = resolve(parse(&["--non-tls-port", "5432,6379"]).expect("args")).expect("config");
    assert_eq!(
        config.non_tls_ports,
        vec![
            NonZeroU16::new(5432).expect("port"),
            NonZeroU16::new(6379).expect("port")
        ]
    );
}

#[test]
fn invalid_values_are_typed_errors() {
    let zero = resolve(parse(&["--non-tls-port", "0"]).expect("args"));
    assert_eq!(zero.expect_err("zero port"), ConfigError::ZeroPort);

    let small = resolve(parse(&["--max-head-bytes", "10"]).expect("args"));
    assert_eq!(small.expect_err("small head"), ConfigError::HeadLimitTooSmall);

    let instant = resolve(parse(&["--first-record-timeout-secs", "0"]).expect("args"));
    assert_eq!(instant.expect_err("timeout"), ConfigError::FirstRecordTimeoutTooSmall);

    let mut args = parse(&[]).expect("args");
    args.trust_domain = "Not A Domain".to_owned();
    assert!(matches!(
        resolve(args).expect_err("domain"),
        ConfigError::TrustDomain(_)
    ));
}
