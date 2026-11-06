use super::*;

#[path = "workload_reply_prefix_tests.rs"]
mod reply_prefix_tests;

const WORKLOAD_ARGS: [&str; 23] = [
    "trogon-egress-proxy",
    "--policy-bucket",
    "EGRESS_POLICY",
    "--policy-incarnation",
    "1",
    "--workload-api-endpoint",
    "unix:///run/spire/sockets/agent.sock",
    "--workload-spiffe-id",
    "spiffe://trogon.example/service/egress",
    "--registration-bucket",
    "WORKLOAD_REGISTRATIONS",
    "--registration-fence",
    "1",
    "--trust-domain",
    "trogon.example",
    "--tenant",
    "acme",
    "--project",
    "payments",
    "--instance",
    "egress-1",
    "--nats-url",
    "tls://broker.example:4222",
];

#[test]
fn workload_identity_configuration_requires_no_static_private_key() {
    Args::try_parse_from(WORKLOAD_ARGS).expect("a complete Workload API profile needs no static TLS files");
}

fn workload_config(arguments: &[&str]) -> Result<Config, ConfigError> {
    let env = InMemoryEnv::new();
    env.set("NATS_CERT", "/run/fixture/client.pem");
    env.set("NATS_KEY", "/run/fixture/client.key");
    Config::resolve(Args::try_parse_from(arguments).expect("profile arguments"), &env)
}

#[test]
fn workload_profile_preserves_exact_registration_scope_and_recovery_fence() {
    let config = workload_config(&WORKLOAD_ARGS).expect("profile");
    let TlsInput::Workload(workload) = config.tls else {
        panic!("the Workload API profile cannot become static TLS");
    };
    let local = workload.identity().local();
    assert_eq!(local.spiffe_id().to_string(), "spiffe://trogon.example/service/egress");
    assert_eq!(local.tenant(), "acme");
    assert_eq!(local.project(), "payments");
    assert_eq!(workload.identity().recovery_fence().get(), 1);
    assert_eq!(workload.registration_bucket(), "WORKLOAD_REGISTRATIONS");
    assert_eq!(workload.registration_key(), "t61636d65.p7061796d656e7473");
}

#[test]
fn workload_profile_cannot_fall_back_to_disk_keys_or_static_policy() {
    for extra in [
        ["--tls-cert", "server.pem"],
        ["--tls-key", "server.key"],
        ["--client-ca", "ca.pem"],
        ["--policy-file", "policy.toml"],
    ] {
        assert!(Args::try_parse_from(WORKLOAD_ARGS.iter().copied().chain(extra)).is_err());
    }
}

#[test]
fn workload_profile_requires_every_registration_field() {
    for option in ["--workload-spiffe-id", "--registration-bucket", "--registration-fence"] {
        let mut arguments = WORKLOAD_ARGS.to_vec();
        let at = arguments
            .iter()
            .position(|argument| *argument == option)
            .expect("option");
        arguments.drain(at..at + 2);
        assert!(Args::try_parse_from(arguments).is_err());
    }
}

#[test]
fn workload_profile_rejects_network_endpoints_wrong_domains_and_wildcard_buckets() {
    for (option, value) in [
        ("--workload-api-endpoint", "tcp://127.0.0.1:8081"),
        ("--workload-spiffe-id", "spiffe://other.example/service/egress"),
        ("--registration-bucket", "REGISTRATIONS.>"),
    ] {
        let mut arguments = WORKLOAD_ARGS;
        let at = arguments
            .iter()
            .position(|argument| *argument == option)
            .expect("option");
        arguments[at + 1] = value;
        assert!(matches!(
            workload_config(&arguments),
            Err(ConfigError::WorkloadIdentity(_))
        ));
    }
}
