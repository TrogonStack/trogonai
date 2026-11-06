use super::*;

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
