use std::sync::Arc;

use async_nats::jetstream;
use chrono::{DateTime, Utc};
use trogon_nats::RequestClient;
use trogon_sandbox_contract::{CredentialId, CredentialProvider, TenantId, TokenId};
use trogon_sandbox_credential_broker::{
    CredentialBroker, CredentialMaterial, CredentialStore, EnvelopeKms, InMemoryTokenStore, InProcessKey,
    JetStreamAuditLogger, NatsKvCredentialStore,
};
use trogon_sandbox_credential_client::CredentialClient;

use crate::constants::{ADMIN_REVOKE_REASON, ALL_PROVIDERS};
use crate::{AdminError, Console};

fn open_store(js: jetstream::Context, key: InProcessKey) -> NatsKvCredentialStore {
    NatsKvCredentialStore::new(js, Arc::new(EnvelopeKms::new(key)))
}

/// Stores `material_json`, the credential's JSON, as a new credential.
pub async fn set(
    js: jetstream::Context,
    key: InProcessKey,
    provider: CredentialProvider,
    tenant: &TenantId,
    credential_id: &CredentialId,
    material_json: &str,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let material: CredentialMaterial =
        serde_json::from_str(material_json.trim()).map_err(|source| AdminError::Parse {
            what: "credential JSON",
            source,
        })?;
    open_store(js, key)
        .put_credential(provider, tenant, credential_id, &material)
        .await
        .map_err(AdminError::step("store credential"))?;
    console.err(format_args!(
        "stored credential {credential_id} for provider {provider}"
    ))
}

pub async fn list(
    js: jetstream::Context,
    key: InProcessKey,
    provider: CredentialProvider,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let rows = open_store(js, key)
        .list_credentials(provider)
        .await
        .map_err(AdminError::step("list credentials"))?;
    if rows.is_empty() {
        return console.out(format_args!("(no credentials for provider {provider})"));
    }
    for row in rows {
        let rotated: DateTime<Utc> = row.last_rotated.into();
        console.out(format_args!(
            "{}  last_rotated={}  version={}",
            row.credential_id,
            rotated.to_rfc3339(),
            row.version
        ))?;
    }
    Ok(())
}

pub async fn rotate(
    js: jetstream::Context,
    key: InProcessKey,
    provider: CredentialProvider,
    tenant: &TenantId,
    credential_id: &CredentialId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let store = open_store(js, key);
    let material = store
        .get_material(provider, tenant, credential_id)
        .await
        .map_err(AdminError::step("load existing credential"))?;
    let version = store
        .rotate_credential(provider, tenant, credential_id, &material)
        .await
        .map_err(AdminError::step("rotate credential"))?;
    console.out(format_args!("rotated {credential_id} to version {version}"))
}

pub async fn revoke_token<C: RequestClient>(
    client: C,
    provider: CredentialProvider,
    tenant: TenantId,
    token_id: TokenId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    CredentialClient::new(client, provider, tenant)
        .revoke(token_id.clone(), ADMIN_REVOKE_REASON)
        .await
        .map_err(AdminError::step("revoke token"))?;
    console.out(format_args!("revoked token {token_id}"))
}

/// What a migration run covers.
pub struct MigrateRequest {
    pub tenant: TenantId,
    /// One provider, or every provider when `None`.
    pub provider: Option<CredentialProvider>,
    pub dry_run: bool,
    /// Only these records. Empty means every legacy record.
    pub only: Vec<CredentialId>,
}

/// Rewrites legacy (no-AAD) credential records in the envelope format.
///
/// Legacy records carry no tenant, so the operator names the owner. Idempotent and resumable:
/// only records still in the legacy format are touched. With `dry_run` nothing is written. The
/// command fails while any record failed or any legacy record remains (after a real run), so a
/// zero exit means legacy reads can be turned off.
pub async fn migrate(
    js: jetstream::Context,
    key: InProcessKey,
    request: MigrateRequest,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let MigrateRequest {
        tenant,
        provider,
        dry_run,
        only,
    } = request;
    let providers: Vec<CredentialProvider> = provider.map_or_else(|| ALL_PROVIDERS.to_vec(), |one| vec![one]);

    let credentials = Arc::new(open_store(js.clone(), key).with_legacy_reads(true));
    let audit = JetStreamAuditLogger::new(js)
        .await
        .map_err(AdminError::step("open audit log"))?;
    let broker = CredentialBroker::new(credentials, Arc::new(InMemoryTokenStore::new()), Arc::new(audit));
    let (mut migrated, mut would, mut failed, mut remaining) = (0, 0, 0, 0);
    for p in providers {
        let report = broker
            .migrate_legacy(p, &tenant, &only, dry_run)
            .await
            .map_err(AdminError::step("migrate credentials"))?;
        for id in &report.migrated {
            console.out(format_args!("migrated {p} {id}"))?;
        }
        for id in &report.would_migrate {
            console.out(format_args!("would migrate {p} {id}"))?;
        }
        for (id, error) in &report.failed {
            console.err(format_args!("failed {p} {id}: {error}"))?;
        }
        migrated += report.migrated.len();
        would += report.would_migrate.len();
        failed += report.failed.len();
        remaining += report.remaining;
    }
    console.out(format_args!(
        "migrated {migrated}, would migrate {would}, failed {failed}, legacy remaining {remaining}{}",
        if dry_run { " (dry run)" } else { "" }
    ))?;
    if failed > 0 {
        return Err(AdminError::MigrationFailed { failed });
    }
    // With named ids the rest may belong to other tenants, so only an unfiltered run proves that
    // legacy reads can be turned off.
    if !dry_run && only.is_empty() && remaining > 0 {
        return Err(AdminError::LegacyRemain { remaining });
    }
    Ok(())
}
