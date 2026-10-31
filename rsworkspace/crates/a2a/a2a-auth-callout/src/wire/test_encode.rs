//! Test-only helpers for signing authorization request JWTs.

use nats_jwt_rs::authorization::{AuthRequest, ClientInfo, ConnectOpts, ServerID};
use nats_jwt_rs::{ClaimType, Claims};
use nkeys::{KeyPair, XKey};

use super::{AUTH_REQUEST_AUDIENCE, AUTH_REQUEST_XKEY_HEADER, ServerAuthRequestClaims, ServerAuthRequestEnvelope};
use crate::admission_keys::{AuthAccount, XkeyPublic};

/// A nats-server of one `AUTH` account, with the signing key and xkey it
/// generates at boot.
pub(crate) struct FixtureServer {
    key: KeyPair,
    xkey: XKey,
    auth_account: AuthAccount,
}

impl FixtureServer {
    pub(crate) fn generate() -> Self {
        let auth_account = AuthAccount::parse(&KeyPair::new_account().public_key()).expect("fixture AUTH account");
        Self::signing_with(KeyPair::new_server(), auth_account)
    }

    pub(crate) fn signing_with(key: KeyPair, auth_account: AuthAccount) -> Self {
        Self {
            key,
            xkey: XKey::new(),
            auth_account,
        }
    }

    pub(crate) fn auth_account(&self) -> &AuthAccount {
        &self.auth_account
    }

    pub(crate) fn id(&self) -> String {
        self.key.public_key()
    }

    pub(crate) fn xkey(&self) -> &XKey {
        &self.xkey
    }

    pub(crate) fn claims(
        &self,
        user: &KeyPair,
        mut patch: impl FnMut(&mut Claims<AuthRequest>),
    ) -> Claims<AuthRequest> {
        let nats = AuthRequest {
            server: ServerID {
                name: self.id(),
                host: "127.0.0.1".into(),
                id: self.id(),
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
        let mut claims: Claims<AuthRequest> = serde_json::from_value(serde_json::json!({
            "aud": AUTH_REQUEST_AUDIENCE,
            "iat": 1,
            "iss": self.id(),
            "jti": "test",
            "sub": self.auth_account.as_str(),
            "nats": nats
        }))
        .expect("fixture authorization request claims");
        patch(&mut claims);
        claims
    }

    pub(crate) fn sign(&self, user: &KeyPair, patch: impl FnMut(&mut Claims<AuthRequest>)) -> String {
        self.claims(user, patch)
            .encode(&self.key)
            .expect("sign authorization request")
    }

    pub(crate) fn seal(
        &self,
        user: &KeyPair,
        callout: &XkeyPublic,
        mut patch: impl FnMut(&mut Claims<AuthRequest>),
    ) -> (Vec<u8>, async_nats::HeaderMap) {
        let server_xkey = self.xkey.public_key();
        let token = self.sign(user, |claims| {
            claims.nats.server.xkey = Some(server_xkey.clone());
            patch(claims);
        });
        let recipient = callout.to_xkey().expect("callout xkey");
        let sealed = self
            .xkey
            .seal(token.as_bytes(), &recipient)
            .expect("seal authorization request");
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(AUTH_REQUEST_XKEY_HEADER, server_xkey.as_str());
        (sealed, headers)
    }

    pub(crate) fn request(
        &self,
        user: &KeyPair,
        patch: impl FnMut(&mut Claims<AuthRequest>),
    ) -> ServerAuthRequestClaims {
        ServerAuthRequestEnvelope::from_bytes(self.sign(user, patch).into_bytes())
            .decode(&self.auth_account, None, None)
            .expect("fixture authorization request decodes")
    }
}
