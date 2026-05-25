//! Test helpers for minting auth-callout User JWTs (dev-dependencies and `test-support` feature).

use std::time::Duration;

use nkeys::KeyPair;
use serde_json::json;

use crate::jwt::{
    AudienceAccount, CallerId, ExternalSubject, MintedUserJwt, SigningKey, SpiceDbPrincipal, UserJwtClaims,
    UserJwtSubject,
};
use crate::permissions::IssuedPermissions;
use crate::signing_key_source::{KeyVersion, MintingMaterial};

/// Mints a short-lived User JWT suitable for gateway ingress header tests.
pub fn mint_test_user_jwt(caller_id: &str, audience_account: &str, ttl: Duration) -> MintedUserJwt {
    let issuer = KeyPair::new_account();
    let issuer_seed = issuer.seed().expect("issuer seed");
    let user = KeyPair::new_user();
    let material = MintingMaterial::new(
        SigningKey::from_seed(&issuer_seed).unwrap().keypair().clone(),
        KeyVersion::new("test").unwrap(),
    );
    let caller = CallerId::new(caller_id).expect("caller_id");
    let claims = UserJwtClaims {
        kid: material.version().clone(),
        sub: ExternalSubject::new("test-sub").expect("external subject"),
        aud: AudienceAccount::new(audience_account),
        data: SpiceDbPrincipal(json!({"spicedb_subject": "test-sub"})),
        nats_permissions: IssuedPermissions::default_for_caller(&caller),
        caller_id: caller,
    };
    let subject = UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
    let issued_at = std::time::SystemTime::now();
    claims.mint(&material, &subject, issued_at, ttl).expect("mint test jwt")
}
