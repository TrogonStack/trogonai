#![allow(clippy::expect_used)]

use a2a_auth_callout::{
    AccountPublicKeyError, IssuerPublicKey, IssuerPublicKeys, IssuerPublicKeysError, VerifyingKeySource,
};
use nkeys::KeyPair;

#[test]
fn issuer_configuration_roundtrips_canonical_public_keys_in_rotation_order() {
    let current = KeyPair::new_account().public_key();
    let previous = KeyPair::new_account().public_key();
    let current_key: IssuerPublicKey = format!("  {current}  ").parse().expect("account public key");
    let previous_key: IssuerPublicKey = previous.parse().expect("previous account public key");
    assert_eq!(current_key.to_string(), current);
    assert_eq!(previous_key.to_string(), previous);

    let keys = IssuerPublicKeys::new([current_key.clone(), previous_key.clone()]).expect("rotation keys");
    let accepted = keys.accepted_verifying_keys();
    let public_keys: Vec<_> = accepted.iter().map(|key| key.issuer_public()).collect();
    assert_eq!(public_keys, [current.as_str(), previous.as_str()]);
    let reloaded = IssuerPublicKeys::parse(&public_keys.join(",")).expect("reload public configuration");
    let reloaded = reloaded.accepted_verifying_keys();
    assert_eq!(
        reloaded.iter().map(|key| key.issuer_public()).collect::<Vec<_>>(),
        public_keys
    );
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "display redaction is the contract under test; typed variants separately prove error classification"
    )
)]
fn issuer_configuration_rejects_signing_material_without_echoing_it() {
    let issuer = KeyPair::new_account();
    let seed = issuer.seed().expect("seed");
    let error = seed
        .parse::<IssuerPublicKey>()
        .expect_err("a seed grants minting authority");
    assert_eq!(error, AccountPublicKeyError::NotAPublicKey);
    assert!(!error.to_string().contains(&seed));

    let error = IssuerPublicKeys::parse(&format!("{}, {seed}", issuer.public_key())).expect_err("seed in list");
    assert!(matches!(error, IssuerPublicKeysError::Invalid { position: 2, .. }));
    assert!(!error.to_string().contains(&seed));
    assert!(!format!("{error:?}").contains(&seed));
    assert_eq!(
        KeyPair::new_user().public_key().parse::<IssuerPublicKey>(),
        Err(AccountPublicKeyError::NotAnAccount)
    );
}
