#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;

const P256_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";

#[test]
fn pop_headers_debug_redacts_agent_and_auth_jwt() {
    let signer = AgentSigner::from_pkcs8_pem(P256_PEM, "agent.jwt.value")
        .expect("valid PKCS8 key")
        .with_auth_token("auth.jwt.value");
    let headers = signer.sign_nats_request("subject.example", None, b"payload", 1000, "nonce-1");
    let debug_output = format!("{headers:?}");
    assert!(
        !debug_output.contains("agent.jwt.value"),
        "leaked agent_jwt: {debug_output}"
    );
    assert!(
        !debug_output.contains("auth.jwt.value"),
        "leaked auth_jwt: {debug_output}"
    );
    assert!(debug_output.contains("<redacted>"));
}
