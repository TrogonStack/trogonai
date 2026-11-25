use tracing::warn;
use trogon_nats::CredentialValue;
use trogon_service_config::SecretReference;
use trogon_std::env::ReadEnv;

use super::AdmissionKeys;
use super::account_public_key::{AuthAccount, TenantAccount};
use super::account_signing_key::AccountSigningKey;
use super::admission_key_error::{AdmissionKeyError, AdmissionKeySlot};
use super::callout_xkey::CalloutXkey;
use super::signing_key_custody::SigningKeyCustody;
use super::tenant_signing_keys::TenantSigningKeys;
use crate::constants::{AUTH_ACCOUNT_VAR, CALLOUT_XKEY_VAR, RESPONSE_SIGNING_KEY_VAR, TENANT_SIGNING_KEYS_VAR};
use crate::error::AuthCalloutError;

pub async fn admission_keys_from_env(env: &impl ReadEnv) -> Result<AdmissionKeys, AuthCalloutError> {
    let custody = SigningKeyCustody::from_env(env)?;
    if custody == SigningKeyCustody::Env {
        warn!(
            "admission seeds held in the process environment are for development only; use file custody in production"
        );
    }
    let auth_account =
        AuthAccount::parse(&required(env, AUTH_ACCOUNT_VAR)?).map_err(AdmissionKeyError::InvalidAuthAccount)?;
    let response_signing_key = account_signing_key(
        env,
        custody,
        AdmissionKeySlot::ResponseSigningKey,
        &required(env, RESPONSE_SIGNING_KEY_VAR)?,
    )
    .await?;
    let tenant_signing_keys = tenant_signing_keys(env, custody, &required(env, TENANT_SIGNING_KEYS_VAR)?).await?;
    let callout_xkey = match env.var(CALLOUT_XKEY_VAR).ok().filter(|raw| !raw.trim().is_empty()) {
        Some(raw) => Some(callout_xkey(env, custody, &raw).await?),
        None => None,
    };
    Ok(AdmissionKeys::new(
        auth_account,
        response_signing_key,
        tenant_signing_keys,
        callout_xkey,
    ))
}

fn required(env: &impl ReadEnv, name: &'static str) -> Result<String, AuthCalloutError> {
    env.var(name).map_err(|_| AuthCalloutError::MissingEnvVar(name))
}

async fn resolve_seed(
    env: &impl ReadEnv,
    custody: SigningKeyCustody,
    slot: &AdmissionKeySlot,
    raw_reference: &str,
) -> Result<String, AdmissionKeyError> {
    let reference =
        raw_reference
            .trim()
            .parse::<SecretReference>()
            .map_err(|source| AdmissionKeyError::InvalidReference {
                slot: slot.clone(),
                source,
            })?;
    if !custody.holds(&reference) {
        return Err(AdmissionKeyError::ReferenceOutsideCustody {
            slot: slot.clone(),
            custody,
        });
    }
    let location = match reference {
        SecretReference::Env(name) => CredentialValue::Env(name.as_str().to_owned()),
        SecretReference::File(path) => CredentialValue::File(path),
    };
    location
        .resolve(env)
        .await
        .map_err(|source| AdmissionKeyError::Unresolvable {
            slot: slot.clone(),
            source,
        })
}

async fn account_signing_key(
    env: &impl ReadEnv,
    custody: SigningKeyCustody,
    slot: AdmissionKeySlot,
    raw_reference: &str,
) -> Result<AccountSigningKey, AdmissionKeyError> {
    let seed = resolve_seed(env, custody, &slot, raw_reference).await?;
    AccountSigningKey::from_seed(&seed).map_err(|source| AdmissionKeyError::InvalidSigningKey { slot, source })
}

async fn tenant_signing_keys(
    env: &impl ReadEnv,
    custody: SigningKeyCustody,
    raw: &str,
) -> Result<TenantSigningKeys, AdmissionKeyError> {
    let mut entries = Vec::new();
    for (index, entry) in raw.split(',').enumerate() {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let position = index + 1;
        let (tenant, reference) = entry
            .split_once('=')
            .ok_or(AdmissionKeyError::MalformedTenantEntry { position })?;
        let tenant = TenantAccount::parse(tenant)
            .map_err(|source| AdmissionKeyError::InvalidTenantAccount { position, source })?;
        let signing_key = account_signing_key(
            env,
            custody,
            AdmissionKeySlot::TenantSigningKey(tenant.clone()),
            reference,
        )
        .await?;
        entries.push((tenant, signing_key));
    }
    TenantSigningKeys::new(entries).map_err(AdmissionKeyError::TenantSigningKeys)
}

async fn callout_xkey(
    env: &impl ReadEnv,
    custody: SigningKeyCustody,
    raw_reference: &str,
) -> Result<CalloutXkey, AdmissionKeyError> {
    let seed = resolve_seed(env, custody, &AdmissionKeySlot::CalloutXkey, raw_reference).await?;
    CalloutXkey::from_seed(&seed).map_err(AdmissionKeyError::InvalidCalloutXkey)
}

#[cfg(test)]
mod tests;
