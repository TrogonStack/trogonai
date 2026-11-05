use super::*;

const REQUIRED: [&str; 11] = [
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
];

fn parse(extra: &[&str]) -> Result<Args, clap::Error> {
    Args::try_parse_from(REQUIRED.iter().chain(extra.iter()))
}

#[test]
fn defaults_are_conservative() {
    let config = Config::try_from(parse(&[]).expect("args")).expect("config");
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
    ];
    assert!(Args::try_parse_from(without_policy).is_err());
}

#[test]
fn non_tls_ports_are_a_comma_separated_list() {
    let config = Config::try_from(parse(&["--non-tls-port", "5432,6379"]).expect("args")).expect("config");
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
    let zero = Config::try_from(parse(&["--non-tls-port", "0"]).expect("args"));
    assert_eq!(zero.expect_err("zero port"), ConfigError::ZeroPort);

    let small = Config::try_from(parse(&["--max-head-bytes", "10"]).expect("args"));
    assert_eq!(small.expect_err("small head"), ConfigError::HeadLimitTooSmall);

    let instant = Config::try_from(parse(&["--first-record-timeout-secs", "0"]).expect("args"));
    assert_eq!(instant.expect_err("timeout"), ConfigError::FirstRecordTimeoutTooSmall);

    let mut args = parse(&[]).expect("args");
    args.trust_domain = "Not A Domain".to_owned();
    assert!(matches!(
        Config::try_from(args).expect_err("domain"),
        ConfigError::TrustDomain(_)
    ));
}
