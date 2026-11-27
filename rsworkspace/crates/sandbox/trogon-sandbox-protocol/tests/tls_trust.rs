use trogon_sandbox_protocol::{CA_TRUST_ENV_VARS, ca_trust_env};

#[test]
fn every_variable_points_at_the_ca_file() {
    let env = ca_trust_env("/etc/ca.pem");
    assert_eq!(env.len(), CA_TRUST_ENV_VARS.len());
    assert!(env.iter().all(|(_, value)| value == "/etc/ca.pem"));
    for name in ["SSL_CERT_FILE", "NODE_EXTRA_CA_CERTS", "GIT_SSL_CAINFO"] {
        assert!(env.iter().any(|(candidate, _)| *candidate == name), "missing {name}");
    }
}

#[test]
fn variable_names_are_unique() {
    let mut names = CA_TRUST_ENV_VARS.to_vec();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CA_TRUST_ENV_VARS.len());
}
