use nkeys::{KeyPair, XKey};
use trogon_service_config::SecretReference;
use trogon_std::env::InMemoryEnv;

use super::*;
use crate::constants::SIGNING_KEY_SOURCE_VAR;
use crate::error::AuthCalloutError;

fn account_signing_key() -> (AccountSigningKey, String) {
    let account = KeyPair::new_account();
    let seed = account.seed().expect("account seed");
    let signing_key = AccountSigningKey::from_seed(&seed).expect("account signing key");
    (signing_key, seed)
}

fn tenant_account() -> TenantAccount {
    TenantAccount::parse(&KeyPair::new_account().public_key()).expect("tenant account")
}

fn callout_xkey() -> (CalloutXkey, String) {
    let seed = XKey::new().seed().expect("curve seed");
    (CalloutXkey::from_seed(&seed).expect("callout xkey"), seed)
}

#[test]
fn account_public_keys_accept_an_account_key_and_drop_surrounding_whitespace() {
    let account = KeyPair::new_account().public_key();

    let tenant: TenantAccount = format!("  {account}\n").parse().expect("tenant account");
    let auth: AuthAccount = account.parse().expect("auth account");

    assert_eq!(tenant.as_str(), account);
    assert_eq!(auth.as_str(), account);
    assert_eq!(tenant.to_string(), account);
    assert_eq!(auth.to_string(), account);
}

#[test]
fn account_public_keys_refuse_keys_of_other_kinds() {
    let user = KeyPair::new_user().public_key();
    let operator = KeyPair::new_operator().public_key();

    assert_eq!(TenantAccount::parse(&user), Err(AccountPublicKeyError::NotAnAccount));
    assert_eq!(AuthAccount::parse(&operator), Err(AccountPublicKeyError::NotAnAccount));
}

#[test]
fn account_public_keys_refuse_names_and_seeds() {
    let seed = KeyPair::new_account().seed().expect("account seed");

    assert_eq!(TenantAccount::parse("DEV"), Err(AccountPublicKeyError::NotAPublicKey));
    assert_eq!(AuthAccount::parse(""), Err(AccountPublicKeyError::NotAPublicKey));
    assert_eq!(TenantAccount::parse(&seed), Err(AccountPublicKeyError::NotAPublicKey));
}

#[test]
fn account_signing_key_loads_an_account_seed() {
    let account = KeyPair::new_account();

    let signing_key = AccountSigningKey::from_seed(&account.seed().expect("account seed")).expect("signing key");

    assert_eq!(signing_key.public_key(), account.public_key());
}

#[test]
fn account_signing_key_refuses_seeds_that_cannot_sign_for_an_account() {
    let user_seed = KeyPair::new_user().seed().expect("user seed");
    let curve_seed = XKey::new().seed().expect("curve seed");

    assert!(matches!(
        AccountSigningKey::from_seed(&user_seed),
        Err(AccountSigningKeyError::NotAnAccountSeed)
    ));
    assert!(matches!(
        AccountSigningKey::from_seed(&curve_seed),
        Err(AccountSigningKeyError::NotAnAccountSeed)
    ));
    assert!(matches!(
        AccountSigningKey::from_seed(&KeyPair::new_account().public_key()),
        Err(AccountSigningKeyError::NotASeed)
    ));
}

#[test]
fn account_signing_key_debug_shows_the_public_key_and_never_the_seed() {
    let (signing_key, seed) = account_signing_key();

    let rendered = format!("{signing_key:?}");

    assert!(rendered.contains(&signing_key.public_key()));
    assert!(!rendered.contains(&seed));
}

#[test]
fn callout_xkey_opens_what_a_sender_sealed_to_it() {
    let (callout, _) = callout_xkey();
    let sender = XKey::new();
    let recipient = callout.public_key().to_xkey().expect("callout public xkey");
    let sealed = sender.seal(b"caller credential", &recipient).expect("seal");

    let opened = callout.open(&sealed, &XkeyPublic::from_xkey(&sender)).expect("open");

    assert_eq!(opened, b"caller credential");
}

#[test]
fn callout_xkey_seals_what_only_the_recipient_opens() {
    let (callout, _) = callout_xkey();
    let recipient = XKey::new();
    let sender = callout.public_key().to_xkey().expect("callout public xkey");

    let sealed = callout
        .seal(b"authorization response", &XkeyPublic::from_xkey(&recipient))
        .expect("seal");

    assert_eq!(
        recipient.open(&sealed, &sender).expect("open"),
        b"authorization response"
    );
    assert!(XKey::new().open(&sealed, &sender).is_err());
}

#[test]
fn callout_xkey_refuses_a_tampered_payload() {
    let (callout, _) = callout_xkey();
    let sender = XKey::new();
    let recipient = callout.public_key().to_xkey().expect("callout public xkey");
    let mut sealed = sender.seal(b"caller credential", &recipient).expect("seal");
    let last = sealed.len() - 1;
    sealed[last] ^= 0x01;

    assert_eq!(
        callout.open(&sealed, &XkeyPublic::from_xkey(&sender)),
        Err(XkeyExchangeError::CannotOpen)
    );
}

#[test]
fn callout_xkey_refuses_a_payload_sealed_to_another_key() {
    let (callout, _) = callout_xkey();
    let sender = XKey::new();
    let sealed = sender.seal(b"caller credential", &XKey::new()).expect("seal");

    assert_eq!(
        callout.open(&sealed, &XkeyPublic::from_xkey(&sender)),
        Err(XkeyExchangeError::CannotOpen)
    );
}

#[test]
fn callout_xkey_refuses_a_seed_that_is_not_a_curve_seed() {
    let account_seed = KeyPair::new_account().seed().expect("account seed");

    assert!(matches!(
        CalloutXkey::from_seed(&account_seed),
        Err(CalloutXkeyError::NotACurveSeed)
    ));
    assert!(matches!(
        CalloutXkey::from_seed("not a seed"),
        Err(CalloutXkeyError::NotACurveSeed)
    ));
}

#[test]
fn callout_xkey_debug_shows_the_public_key_and_never_the_seed() {
    let (callout, seed) = callout_xkey();

    let rendered = format!("{callout:?}");

    assert!(rendered.contains(callout.public_key().as_str()));
    assert!(!rendered.contains(&seed));
}

#[test]
fn tenant_signing_keys_find_the_key_registered_for_each_tenant() {
    let tenant = tenant_account();
    let (signing_key, _) = account_signing_key();
    let expected = signing_key.public_key();

    let keys = TenantSigningKeys::new([(tenant.clone(), signing_key)]).expect("tenant signing keys");

    assert_eq!(
        keys.signing_key(&tenant).map(AccountSigningKey::public_key),
        Some(expected)
    );
    assert_eq!(keys.tenants().collect::<Vec<_>>(), vec![&tenant]);
    assert!(keys.signing_key(&tenant_account()).is_none());
}

#[test]
fn tenant_signing_keys_refuse_an_empty_list() {
    assert!(matches!(
        TenantSigningKeys::new(std::iter::empty()),
        Err(TenantSigningKeysError::Empty)
    ));
}

#[test]
fn tenant_signing_keys_refuse_a_tenant_listed_twice() {
    let tenant = tenant_account();
    let (first, _) = account_signing_key();
    let (second, _) = account_signing_key();

    assert!(matches!(
        TenantSigningKeys::new([(tenant.clone(), first), (tenant.clone(), second)]),
        Err(TenantSigningKeysError::Duplicate(listed)) if listed == tenant
    ));
}

#[test]
fn tenant_signing_keys_debug_never_shows_a_seed() {
    let tenant = tenant_account();
    let (signing_key, seed) = account_signing_key();
    let keys = TenantSigningKeys::new([(tenant.clone(), signing_key)]).expect("tenant signing keys");

    let rendered = format!("{keys:?}");

    assert!(rendered.contains(tenant.as_str()));
    assert!(!rendered.contains(&seed));
}

#[test]
fn custody_defaults_to_env_when_the_source_is_unset() {
    assert!(matches!(
        SigningKeyCustody::from_env(&InMemoryEnv::new()),
        Ok(SigningKeyCustody::Env)
    ));
}

#[test]
fn custody_reads_the_env_and_file_sources() {
    let env = InMemoryEnv::new();
    env.set(SIGNING_KEY_SOURCE_VAR, "env");
    assert!(matches!(SigningKeyCustody::from_env(&env), Ok(SigningKeyCustody::Env)));

    env.set(SIGNING_KEY_SOURCE_VAR, "file");
    assert!(matches!(SigningKeyCustody::from_env(&env), Ok(SigningKeyCustody::File)));
    assert_eq!(SigningKeyCustody::Env.to_string(), "env");
    assert_eq!(SigningKeyCustody::File.to_string(), "file");
}

#[test]
fn a_tenant_signing_key_slot_names_its_account() {
    let tenant = tenant_account();

    assert_eq!(AdmissionKeySlot::CalloutXkey.to_string(), "AUTH_CALLOUT_XKEY_SEED");
    assert_eq!(
        AdmissionKeySlot::TenantSigningKey(tenant.clone()).to_string(),
        format!("AUTH_CALLOUT_TENANT_SIGNING_KEYS entry for {tenant}")
    );
}

#[test]
fn custody_refuses_vault_until_it_is_wired() {
    let env = InMemoryEnv::new();
    env.set(SIGNING_KEY_SOURCE_VAR, "vault");

    assert!(matches!(
        SigningKeyCustody::from_env(&env),
        Err(AuthCalloutError::VaultNotConfigured)
    ));
}

#[test]
fn custody_refuses_a_source_it_does_not_know() {
    let env = InMemoryEnv::new();
    env.set(SIGNING_KEY_SOURCE_VAR, "kms");

    assert!(matches!(
        SigningKeyCustody::from_env(&env),
        Err(AuthCalloutError::UnknownSigningKeySource(source)) if source == "kms"
    ));
}

#[test]
fn custody_holds_only_references_of_its_own_kind() {
    let env_reference: SecretReference = "env://AUTH_CALLOUT_SEED".parse().expect("env reference");
    let file_reference: SecretReference = "file:///run/secrets/callout.nk".parse().expect("file reference");

    assert!(SigningKeyCustody::Env.holds(&env_reference));
    assert!(!SigningKeyCustody::Env.holds(&file_reference));
    assert!(SigningKeyCustody::File.holds(&file_reference));
    assert!(!SigningKeyCustody::File.holds(&env_reference));
}

#[test]
fn admission_keys_expose_what_they_were_built_from() {
    let auth_account = AuthAccount::parse(&KeyPair::new_account().public_key()).expect("auth account");
    let (response_signing_key, _) = account_signing_key();
    let response_public = response_signing_key.public_key();
    let tenant = tenant_account();
    let (tenant_key, _) = account_signing_key();
    let tenant_signing_keys = TenantSigningKeys::new([(tenant.clone(), tenant_key)]).expect("tenant signing keys");
    let (callout, _) = callout_xkey();
    let callout_public = callout.public_key();

    let keys = AdmissionKeys::new(
        auth_account.clone(),
        response_signing_key,
        tenant_signing_keys,
        Some(callout),
    );

    assert_eq!(keys.auth_account(), &auth_account);
    assert_eq!(keys.response_signing_key().public_key(), response_public);
    assert!(keys.tenant_signing_keys().signing_key(&tenant).is_some());
    assert_eq!(keys.callout_xkey().map(CalloutXkey::public_key), Some(callout_public));
}
