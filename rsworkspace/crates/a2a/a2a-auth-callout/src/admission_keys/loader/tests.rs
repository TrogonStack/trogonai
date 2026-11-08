use std::io::Write;

use nkeys::{KeyPair, XKey};
use tempfile::NamedTempFile;
use trogon_nats::SettingError;
use trogon_service_config::SecretReferenceError;
use trogon_std::env::InMemoryEnv;

use super::admission_keys_from_env;
use crate::admission_keys::{
    AccountPublicKeyError, AccountSigningKey, AccountSigningKeyError, AdmissionKeyError, AdmissionKeySlot,
    CalloutXkeyError, SigningKeyCustody, TenantAccount, TenantSigningKeysError,
};
use crate::constants::{
    AUTH_ACCOUNT_VAR, CALLOUT_XKEY_VAR, RESPONSE_SIGNING_KEY_VAR, SIGNING_KEY_SOURCE_VAR, TENANT_SIGNING_KEYS_VAR,
};
use crate::error::AuthCalloutError;

struct Deployment {
    auth_account: KeyPair,
    response_signing_key: KeyPair,
    tenant: KeyPair,
    tenant_signing_key: KeyPair,
    xkey: XKey,
}

impl Deployment {
    fn generate() -> Self {
        Self {
            auth_account: KeyPair::new_account(),
            response_signing_key: KeyPair::new_account(),
            tenant: KeyPair::new_account(),
            tenant_signing_key: KeyPair::new_account(),
            xkey: XKey::new(),
        }
    }

    fn tenant_account(&self) -> TenantAccount {
        TenantAccount::parse(&self.tenant.public_key()).expect("tenant account")
    }

    fn env_custody(&self) -> InMemoryEnv {
        let env = InMemoryEnv::new();
        env.set(SIGNING_KEY_SOURCE_VAR, "env");
        env.set(AUTH_ACCOUNT_VAR, self.auth_account.public_key());
        env.set("AUTH_RESPONSE_SEED", seed(&self.response_signing_key));
        env.set("DEV_SIGNING_SEED", seed(&self.tenant_signing_key));
        env.set("CALLOUT_CURVE_SEED", self.xkey.seed().expect("curve seed"));
        env.set(RESPONSE_SIGNING_KEY_VAR, "env://AUTH_RESPONSE_SEED");
        env.set(
            TENANT_SIGNING_KEYS_VAR,
            format!("{}=env://DEV_SIGNING_SEED", self.tenant.public_key()),
        );
        env.set(CALLOUT_XKEY_VAR, "env://CALLOUT_CURVE_SEED");
        env
    }
}

fn seed(key: &KeyPair) -> String {
    key.seed().expect("account seed")
}

fn seed_file(seed: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("seed file");
    writeln!(file, "{seed}").expect("write seed");
    file
}

fn file_reference(file: &NamedTempFile) -> String {
    format!("file://{}", file.path().display())
}

async fn refusal(env: &InMemoryEnv) -> AuthCalloutError {
    admission_keys_from_env(env)
        .await
        .expect_err("the admission keys must be refused")
}

#[tokio::test]
async fn file_custody_loads_every_seed_from_its_file() {
    let deployment = Deployment::generate();
    let response_file = seed_file(&seed(&deployment.response_signing_key));
    let tenant_file = seed_file(&seed(&deployment.tenant_signing_key));
    let xkey_file = seed_file(&deployment.xkey.seed().expect("curve seed"));
    let env = InMemoryEnv::new();
    env.set(SIGNING_KEY_SOURCE_VAR, "file");
    env.set(AUTH_ACCOUNT_VAR, deployment.auth_account.public_key());
    env.set(RESPONSE_SIGNING_KEY_VAR, file_reference(&response_file));
    env.set(
        TENANT_SIGNING_KEYS_VAR,
        format!("{}={}", deployment.tenant.public_key(), file_reference(&tenant_file)),
    );
    env.set(CALLOUT_XKEY_VAR, file_reference(&xkey_file));

    let keys = admission_keys_from_env(&env).await.expect("admission keys");

    assert_eq!(keys.auth_account().as_str(), deployment.auth_account.public_key());
    assert_eq!(
        keys.response_signing_key().public_key(),
        deployment.response_signing_key.public_key()
    );
    assert_eq!(
        keys.tenant_signing_keys()
            .signing_key(&deployment.tenant_account())
            .map(AccountSigningKey::public_key),
        Some(deployment.tenant_signing_key.public_key())
    );
    assert_eq!(
        keys.callout_xkey().map(|xkey| xkey.public_key().as_str().to_owned()),
        Some(deployment.xkey.public_key())
    );
}

#[tokio::test]
async fn env_custody_loads_every_seed_from_the_variable_it_names() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();

    let keys = admission_keys_from_env(&env).await.expect("admission keys");

    assert_eq!(
        keys.response_signing_key().public_key(),
        deployment.response_signing_key.public_key()
    );
    assert!(
        keys.tenant_signing_keys()
            .signing_key(&deployment.tenant_account())
            .is_some()
    );
    assert!(keys.callout_xkey().is_some());
}

#[tokio::test]
async fn custody_defaults_to_env_when_the_source_is_unset() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.remove(SIGNING_KEY_SOURCE_VAR);

    assert!(admission_keys_from_env(&env).await.is_ok());
}

#[tokio::test]
async fn several_tenants_each_get_their_own_signing_key() {
    let deployment = Deployment::generate();
    let other_tenant = KeyPair::new_account();
    let other_signing_key = KeyPair::new_account();
    let env = deployment.env_custody();
    env.set("OTHER_SIGNING_SEED", seed(&other_signing_key));
    env.set(
        TENANT_SIGNING_KEYS_VAR,
        format!(
            " {}=env://DEV_SIGNING_SEED , ,{}=env://OTHER_SIGNING_SEED,",
            deployment.tenant.public_key(),
            other_tenant.public_key()
        ),
    );

    let keys = admission_keys_from_env(&env).await.expect("admission keys");
    let other = TenantAccount::parse(&other_tenant.public_key()).expect("tenant account");

    assert_eq!(keys.tenant_signing_keys().tenants().count(), 2);
    assert_eq!(
        keys.tenant_signing_keys()
            .signing_key(&other)
            .map(AccountSigningKey::public_key),
        Some(other_signing_key.public_key())
    );
}

#[tokio::test]
async fn the_callout_xkey_is_optional() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.remove(CALLOUT_XKEY_VAR);

    let keys = admission_keys_from_env(&env).await.expect("admission keys");
    assert!(keys.callout_xkey().is_none());

    env.set(CALLOUT_XKEY_VAR, "  ");
    let keys = admission_keys_from_env(&env).await.expect("admission keys");
    assert!(keys.callout_xkey().is_none());
}

#[tokio::test]
async fn a_seed_written_in_place_of_a_reference_is_refused() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.set(RESPONSE_SIGNING_KEY_VAR, seed(&deployment.response_signing_key));

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::InvalidReference {
            slot: AdmissionKeySlot::ResponseSigningKey,
            source: SecretReferenceError::NotAReference,
        })
    ));
}

#[tokio::test]
async fn a_reference_outside_the_configured_custody_is_refused() {
    let deployment = Deployment::generate();
    let tenant_file = seed_file(&seed(&deployment.tenant_signing_key));
    let env = deployment.env_custody();
    env.set(
        TENANT_SIGNING_KEYS_VAR,
        format!("{}={}", deployment.tenant.public_key(), file_reference(&tenant_file)),
    );

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::ReferenceOutsideCustody {
            slot: AdmissionKeySlot::TenantSigningKey(tenant),
            custody: SigningKeyCustody::Env,
        }) if tenant == deployment.tenant_account()
    ));
}

#[tokio::test]
async fn a_reference_to_an_unset_variable_is_refused() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.remove("CALLOUT_CURVE_SEED");

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::Unresolvable {
            slot: AdmissionKeySlot::CalloutXkey,
            source: SettingError::EnvUnset(name),
        }) if name == "CALLOUT_CURVE_SEED"
    ));
}

#[tokio::test]
async fn a_reference_to_a_missing_file_is_refused() {
    let deployment = Deployment::generate();
    let response_file = seed_file(&seed(&deployment.response_signing_key));
    let reference = file_reference(&response_file);
    drop(response_file);
    let env = deployment.env_custody();
    env.set(SIGNING_KEY_SOURCE_VAR, "file");
    env.set(RESPONSE_SIGNING_KEY_VAR, reference);

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::Unresolvable {
            slot: AdmissionKeySlot::ResponseSigningKey,
            source: SettingError::Read { .. },
        })
    ));
}

#[tokio::test]
async fn every_required_variable_must_be_set() {
    for name in [AUTH_ACCOUNT_VAR, RESPONSE_SIGNING_KEY_VAR, TENANT_SIGNING_KEYS_VAR] {
        let env = Deployment::generate().env_custody();
        env.remove(name);

        assert!(
            matches!(refusal(&env).await, AuthCalloutError::MissingEnvVar(missing) if missing == name),
            "{name} must be required"
        );
    }
}

#[tokio::test]
async fn the_auth_account_must_be_an_account_public_key() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.set(AUTH_ACCOUNT_VAR, KeyPair::new_user().public_key());

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::InvalidAuthAccount(
            AccountPublicKeyError::NotAnAccount
        ))
    ));
}

#[tokio::test]
async fn a_signing_seed_that_is_not_an_account_seed_is_refused() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.set("AUTH_RESPONSE_SEED", seed(&KeyPair::new_user()));

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::InvalidSigningKey {
            slot: AdmissionKeySlot::ResponseSigningKey,
            source: AccountSigningKeyError::NotAnAccountSeed,
        })
    ));
}

#[tokio::test]
async fn a_callout_xkey_that_is_not_a_curve_seed_is_refused() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.set("CALLOUT_CURVE_SEED", seed(&KeyPair::new_account()));

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::InvalidCalloutXkey(CalloutXkeyError::NotACurveSeed))
    ));
}

#[tokio::test]
async fn a_tenant_entry_without_a_reference_is_refused_by_position() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.set(
        TENANT_SIGNING_KEYS_VAR,
        format!(
            "{}=env://DEV_SIGNING_SEED,{}",
            deployment.tenant.public_key(),
            KeyPair::new_account().public_key()
        ),
    );

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::MalformedTenantEntry { position: 2 })
    ));
}

#[tokio::test]
async fn a_tenant_entry_naming_no_account_is_refused_by_position() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.set(TENANT_SIGNING_KEYS_VAR, "DEV=env://DEV_SIGNING_SEED");

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::InvalidTenantAccount {
            position: 1,
            source: AccountPublicKeyError::NotAPublicKey,
        })
    ));
}

#[tokio::test]
async fn a_tenant_listed_twice_is_refused() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.set(
        TENANT_SIGNING_KEYS_VAR,
        format!(
            "{tenant}=env://DEV_SIGNING_SEED,{tenant}=env://DEV_SIGNING_SEED",
            tenant = deployment.tenant.public_key()
        ),
    );

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::TenantSigningKeys(TenantSigningKeysError::Duplicate(
            tenant
        ))) if tenant == deployment.tenant_account()
    ));
}

#[tokio::test]
async fn a_tenant_list_with_no_entries_is_refused() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.set(TENANT_SIGNING_KEYS_VAR, " , ");

    assert!(matches!(
        refusal(&env).await,
        AuthCalloutError::AdmissionKeys(AdmissionKeyError::TenantSigningKeys(TenantSigningKeysError::Empty))
    ));
}

#[tokio::test]
async fn vault_custody_is_refused_until_it_is_wired() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    env.set(SIGNING_KEY_SOURCE_VAR, "vault");

    assert!(matches!(refusal(&env).await, AuthCalloutError::VaultNotConfigured));
}

#[tokio::test]
async fn refusals_never_echo_a_seed() {
    let deployment = Deployment::generate();
    let env = deployment.env_custody();
    let leaked = seed(&deployment.response_signing_key);
    env.set(RESPONSE_SIGNING_KEY_VAR, leaked.clone());

    let refusal = refusal(&env).await;

    let rendered = format!("{refusal} {refusal:?}");
    assert!(!rendered.contains(&leaked));
}
