//! Test-only helpers for signing authorization request JWTs.

use nats_jwt_rs::authorization::{AuthRequest, ClientInfo, ConnectOpts, ServerID};
use nats_jwt_rs::{ClaimType, Claims};
use nkeys::KeyPair;

use super::AUTH_REQUEST_AUDIENCE;

pub(crate) fn signed_auth_request(
    server: &KeyPair,
    user: &KeyPair,
    mut patch: impl FnMut(&mut Claims<AuthRequest>),
) -> String {
    let nats = AuthRequest {
        server: ServerID {
            name: server.public_key(),
            host: "127.0.0.1".into(),
            id: server.public_key(),
            version: Some("2.14.1".into()),
            cluster: None,
            tags: None,
            xkey: None,
        },
        user_nkey: user.public_key(),
        client_info: ClientInfo {
            host: "127.0.0.1".into(),
            id: 1,
            user: "tenant-acme".into(),
            name: None,
            tags: None,
            name_tag: String::new(),
            kind: "Client".into(),
            client_type: "nats".into(),
            mqtt: None,
            nonce: String::new(),
        },
        connect_opts: ConnectOpts {
            jwt: Some("opaque.jwt.token".into()),
            nkey: None,
            sig: None,
            auth_token: None,
            user: Some("tenant-acme".into()),
            pass: None,
            name: None,
            lang: None,
            version: None,
            protocol: 1,
        },
        client_tls: None,
        request_nonce: None,
        generic_fields: nats_jwt_rs::types::GenericFields {
            claim_type: ClaimType::AuthorizationRequest,
            version: 2,
            ..Default::default()
        },
    };
    let mut claim: Claims<AuthRequest> = serde_json::from_value(serde_json::json!({
        "aud": AUTH_REQUEST_AUDIENCE,
        "iat": 1,
        "iss": server.public_key(),
        "jti": "test",
        "sub": server.public_key(),
        "nats": nats
    }))
    .expect("fixture authorization request claims");
    patch(&mut claim);
    claim.encode(server).expect("sign authorization request")
}
