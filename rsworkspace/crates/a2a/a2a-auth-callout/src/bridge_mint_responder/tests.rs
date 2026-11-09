use std::sync::Arc;
use std::time::Duration;

use a2a_nats::A2aPrefix;
use nkeys::{KeyPair, XKey};
use serde_json::{Value, json};
use trogon_identity_types::PrincipalKind;
use trogon_std::env::InMemoryEnv;

use super::*;
use crate::VerifiedCredential;
use crate::admission_keys::{AccountSigningKey, TenantAccount, TenantSigningKeys};
use crate::bridge_mint::{BridgeClientInfo, BridgeCredential, SealedBridgeCredential};
use crate::credentials::mtls::{ClientCertPem, MTlsVerifier};
use crate::credentials::oidc::{BearerToken, OidcVerifier};
use crate::dispatcher::{CalloutDispatcher, CalloutDispatcherConfig};
use crate::jwt::{
    AudienceAccount, CallerId, ExternalSubject, SpiceDbPrincipal, UserJwtClaims, decode_nats_user_payload,
};
use crate::permissions::IssuedPermissions;
use crate::tenant_admission::{TenantMembership, TenantMemberships};

const GOOD_TOKEN: &str = "good.bearer.token";

struct TokenVerifier;

#[async_trait::async_trait]
impl OidcVerifier for TokenVerifier {
    async fn verify(
        &self,
        token: &BearerToken,
        account: &AudienceAccount,
    ) -> Result<VerifiedCredential, AuthCalloutError> {
        if token.as_str() != GOOD_TOKEN {
            return Err(CredentialError::InvalidCredentials("verifier diagnostic detail".into()).into());
        }
        let caller = CallerId::new("alice").unwrap();
        Ok(crate::test_support::verified_test_credential(UserJwtClaims {
            kid: crate::signing_key_source::unminted_placeholder(),
            sub: ExternalSubject::new("alice-sub").unwrap(),
            aud: account.clone(),
            data: SpiceDbPrincipal(json!({"spicedb_subject": "alice-sub"})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller),
            caller_id: caller,
            principal_kind: PrincipalKind::Person,
        }))
    }
}

/// Accepts any certificate, so a test that expects a refusal shows the
/// responder never asked it.
struct AcceptingMtlsVerifier;

#[async_trait::async_trait]
impl MTlsVerifier for AcceptingMtlsVerifier {
    async fn verify(
        &self,
        _cert: &ClientCertPem,
        account: &AudienceAccount,
    ) -> Result<VerifiedCredential, AuthCalloutError> {
        let caller = CallerId::new("svc").unwrap();
        Ok(crate::test_support::verified_test_credential(UserJwtClaims {
            kid: crate::signing_key_source::unminted_placeholder(),
            sub: ExternalSubject::new("CN=svc").unwrap(),
            aud: account.clone(),
            data: SpiceDbPrincipal(json!({"spicedb_subject": "svc"})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller),
            caller_id: caller,
            principal_kind: PrincipalKind::Service,
        }))
    }
}

struct Fixture {
    responder: BridgeMintResponder<Arc<CalloutDispatcher>>,
    tenant: TenantAccount,
    callout: crate::admission_keys::CalloutXkey,
    signing_public: String,
}

impl Fixture {
    fn new() -> Self {
        Self::with_xkey(true)
    }

    fn with_xkey(configure_xkey: bool) -> Self {
        let tenant = TenantAccount::parse(&KeyPair::new_account().public_key()).unwrap();
        let signing_key = AccountSigningKey::from_seed(&KeyPair::new_account().seed().unwrap()).unwrap();
        let signing_public = signing_key.public_key();
        let dispatcher = CalloutDispatcher::new(CalloutDispatcherConfig {
            tenant_signing_keys: TenantSigningKeys::new([(tenant.clone(), signing_key)]).unwrap(),
            tenant_admission: Arc::new(
                TenantMemberships::new([
                    TenantMembership::new(
                        tenant.clone(),
                        trogon_identity_types::SubjectPrincipal::from_tokens("person", "alice").unwrap(),
                    ),
                    TenantMembership::new(
                        tenant.clone(),
                        trogon_identity_types::SubjectPrincipal::from_tokens("service", "svc").unwrap(),
                    ),
                ])
                .unwrap(),
            ),
            user_jwt_ttl: Duration::from_secs(60),
            a2a_prefix: A2aPrefix::new("a2a").unwrap(),
            oidc: Some(Arc::new(TokenVerifier)),
            mtls: Some(Arc::new(AcceptingMtlsVerifier)),
            api_key: None,
        });
        let callout = crate::admission_keys::CalloutXkey::from_seed(&XKey::new().seed().unwrap()).unwrap();
        Self {
            responder: BridgeMintResponder::new(Arc::new(dispatcher), configure_xkey.then(|| callout.clone())),
            tenant,
            callout,
            signing_public,
        }
    }

    fn sealed(&self, token: &str) -> SealedBridgeCredential {
        let credential = BridgeCredential::BearerToken(BearerToken::new(token));
        SealedBridgeCredential::seal(&credential, &self.callout.public_key()).unwrap()
    }

    fn request(&self, token: &str) -> BridgeMintRequest {
        BridgeMintRequest {
            account: Some(self.tenant.as_str().to_owned()),
            sealed_credential: Some(self.sealed(token)),
            ..BridgeMintRequest::default()
        }
    }

    async fn respond(&self, request: &BridgeMintRequest) -> BridgeMintReply {
        self.responder.respond(&serde_json::to_vec(request).unwrap()).await
    }
}

fn refusal(reply: BridgeMintReply) -> String {
    match reply {
        BridgeMintReply::Refused(refusal) => refusal.refusal,
        BridgeMintReply::Minted(_) => panic!("expected a refusal"),
    }
}

fn assert_carries_no_jwt(reply: &BridgeMintReply) {
    let document: Value = serde_json::from_slice(&reply.to_payload()).unwrap();
    assert!(document.get("user_jwt").is_none());
}

#[tokio::test]
async fn a_verified_sealed_credential_mints_a_jwt_with_the_bridge_grants() {
    let fixture = Fixture::new();
    let user = KeyPair::new_user();
    let request = BridgeMintRequest {
        user_nkey: Some(user.public_key()),
        ..fixture.request(GOOD_TOKEN)
    };
    let BridgeMintReply::Minted(minted) = fixture.respond(&request).await else {
        panic!("expected a minted JWT");
    };
    let payload = decode_nats_user_payload(&minted.user_jwt).unwrap();
    assert_eq!(payload["iss"], fixture.signing_public);
    assert_eq!(payload["nats"]["issuer_account"], fixture.tenant.as_str());
    assert_eq!(payload["sub"], user.public_key());
    assert_eq!(
        payload["nats"]["pub"]["allow"],
        json!(["a2a.v1.gateway.*.person.alice.>"])
    );
    assert_eq!(
        payload["nats"]["sub"]["allow"],
        json!([
            "a2a.v1.gateway.person.alice.events",
            "a2a.v1.tasks.*.person.alice.events",
            "_INBOX.>"
        ])
    );
}

#[tokio::test]
async fn a_credential_the_verifier_refuses_gets_an_opaque_refusal_and_no_jwt() {
    let fixture = Fixture::new();
    let reply = fixture.respond(&fixture.request("forged.bearer.token")).await;
    assert_carries_no_jwt(&reply);
    let payload = String::from_utf8(reply.to_payload()).unwrap();
    assert_eq!(payload, r#"{"refusal":"invalid_credentials"}"#);
}

#[tokio::test]
async fn a_malformed_payload_is_refused_as_an_invalid_request() {
    let fixture = Fixture::new();
    for payload in [&b"not json"[..], b"", br#"{"sealed_credential":"nope"}"#] {
        let reply = fixture.responder.respond(payload).await;
        assert_carries_no_jwt(&reply);
        assert_eq!(refusal(reply), "invalid_request");
    }
}

#[tokio::test]
async fn a_plaintext_credential_is_refused_even_next_to_a_valid_sealed_one() {
    let fixture = Fixture::new();
    let mut document = serde_json::to_value(fixture.request(GOOD_TOKEN)).unwrap();
    document["user_jwt"] = json!(GOOD_TOKEN);
    let reply = fixture.responder.respond(&serde_json::to_vec(&document).unwrap()).await;
    assert_eq!(refusal(reply), "invalid_request");
}

#[tokio::test]
async fn a_request_without_a_sealed_credential_mints_nothing_even_with_a_client_certificate() {
    let fixture = Fixture::new();
    let request = BridgeMintRequest {
        account: Some(fixture.tenant.as_str().to_owned()),
        client_info: Some(BridgeClientInfo {
            client_cert_pem: Some("-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----".to_owned()),
        }),
        ..BridgeMintRequest::default()
    };
    let reply = fixture.respond(&request).await;
    assert_carries_no_jwt(&reply);
    assert_eq!(refusal(reply), "invalid_request");
}

#[tokio::test]
async fn a_credential_sealed_to_another_key_is_refused() {
    let fixture = Fixture::new();
    let other = crate::admission_keys::CalloutXkey::from_seed(&XKey::new().seed().unwrap()).unwrap();
    let credential = BridgeCredential::BearerToken(BearerToken::new(GOOD_TOKEN));
    let request = BridgeMintRequest {
        sealed_credential: Some(SealedBridgeCredential::seal(&credential, &other.public_key()).unwrap()),
        ..fixture.request(GOOD_TOKEN)
    };
    assert_eq!(refusal(fixture.respond(&request).await), "invalid_request");
}

#[tokio::test]
async fn a_callout_without_an_xkey_refuses_every_sealed_credential() {
    let fixture = Fixture::with_xkey(false);
    assert_eq!(
        refusal(fixture.respond(&fixture.request(GOOD_TOKEN)).await),
        "invalid_request"
    );
}

#[tokio::test]
async fn a_tenant_the_callout_does_not_serve_is_refused() {
    let fixture = Fixture::new();
    let request = BridgeMintRequest {
        account: Some(KeyPair::new_account().public_key()),
        ..fixture.request(GOOD_TOKEN)
    };
    assert_eq!(refusal(fixture.respond(&request).await), "unknown_account");
}

#[tokio::test]
async fn a_request_naming_no_tenant_is_refused() {
    let fixture = Fixture::new();
    let request = BridgeMintRequest {
        account: None,
        ..fixture.request(GOOD_TOKEN)
    };
    assert_eq!(refusal(fixture.respond(&request).await), "invalid_request");
}

#[test]
fn the_mint_subject_is_the_bridge_default_unless_one_is_configured() {
    let env = InMemoryEnv::new();
    assert_eq!(bridge_mint_subject(&env), "a2a.bridge.auth.callout.request");
    env.set(BRIDGE_MINT_SUBJECT_VAR, "  acme.bridge.mint  ");
    assert_eq!(bridge_mint_subject(&env), "acme.bridge.mint");
    env.set(BRIDGE_MINT_SUBJECT_VAR, "   ");
    assert_eq!(bridge_mint_subject(&env), DEFAULT_BRIDGE_MINT_SUBJECT);
}
