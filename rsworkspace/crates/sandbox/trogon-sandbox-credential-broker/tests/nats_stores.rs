use std::collections::HashSet;
use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use bytes::Bytes;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CredentialAuditEvent, CredentialError, CredentialId, CredentialProvider, CredentialScope, LeaseId, PullSecretId,
    ShortLivedToken, TenantId, TokenId,
};
use trogon_sandbox_credential_broker::{
    AuditLogger, CommittedPolicyStore, CommittedPolicyWriter, CredentialBroker, CredentialMaterial, CredentialStore,
    CredentialStoreError, FenceMode, InProcessKey, JetStreamAuditLogger, KmsProvider, NatsKvCommittedPolicies,
    NatsKvCredentialStore, NatsKvTokenStore, PolicyStoreError, TokenStore,
};
use trogon_sandbox_protocol::subjects::{credential_audit_stream_name, credential_kv_bucket};

const REGISTRY: CredentialProvider = CredentialProvider::Registry;

fn kms() -> Arc<InProcessKey> {
    Arc::new(InProcessKey::from_bytes([5u8; 32]))
}

fn tenant() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("tenant-a")?)
}

fn material(password: &str) -> CredentialMaterial {
    CredentialMaterial::RegistryBasic {
        username: "u".to_string(),
        password: password.to_string(),
    }
}

fn scope() -> CredentialScope {
    CredentialScope::RegistryPull {
        repository_glob: "r/*".to_string(),
    }
}

async fn credential_store() -> Result<(JetStreamTestServer, NatsKvCredentialStore, jetstream::Context), Box<dyn Error>>
{
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    Ok((server, NatsKvCredentialStore::new(js.clone(), kms()), js))
}

#[tokio::test]
async fn credentials_are_sealed_read_rotated_and_listed() -> Result<(), Box<dyn Error>> {
    let (_server, store, js) = credential_store().await?;
    let id = CredentialId::new("c1");
    assert_eq!(
        store.put_credential(REGISTRY, &tenant()?, &id, &material("v1")).await?,
        1
    );
    assert_eq!(store.get_material(REGISTRY, &tenant()?, &id).await?, material("v1"));
    assert!(matches!(
        store.get_material(REGISTRY, &TenantId::new("other")?, &id).await,
        Err(CredentialStoreError::Kms(_))
    ));
    assert!(matches!(
        store
            .get_material(REGISTRY, &tenant()?, &CredentialId::new("missing"))
            .await,
        Err(CredentialStoreError::NotFound)
    ));
    assert_eq!(
        store
            .rotate_credential(REGISTRY, &tenant()?, &id, &material("v2"))
            .await?,
        2
    );
    assert_eq!(
        store
            .rotate_credential(REGISTRY, &tenant()?, &CredentialId::new("fresh"), &material("f"))
            .await?,
        1
    );
    assert_eq!(store.get_material(REGISTRY, &tenant()?, &id).await?, material("v2"));
    assert!(store.list_legacy(REGISTRY).await?.is_empty());
    let mut listed = store.list_credentials(REGISTRY).await?;
    listed.sort_by(|a, b| a.credential_id.as_str().cmp(b.credential_id.as_str()));
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].credential_id, id);
    assert_eq!(listed[0].version, 2);

    store.map_pull_secret(REGISTRY, &PullSecretId::new("pull"), &id).await?;
    assert_eq!(
        store.resolve_pull_secret(REGISTRY, &PullSecretId::new("pull")).await?,
        id
    );
    assert_eq!(store.resolve_pull_secret(REGISTRY, &PullSecretId::new("c1")).await?, id);
    assert_eq!(store.list_credentials(REGISTRY).await?.len(), 2);

    let bucket = js.get_key_value(credential_kv_bucket(REGISTRY)).await?;
    bucket.put("pull_secret.bad", Bytes::from_static(b"{")).await?;
    assert!(matches!(
        store.resolve_pull_secret(REGISTRY, &PullSecretId::new("bad")).await,
        Err(CredentialStoreError::Codec(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_corrupt_record_is_a_codec_error() -> Result<(), Box<dyn Error>> {
    let (_server, store, js) = credential_store().await?;
    store
        .put_credential(REGISTRY, &tenant()?, &CredentialId::new("seed"), &material("v"))
        .await?;
    let bucket = js.get_key_value(credential_kv_bucket(REGISTRY)).await?;
    bucket.put("broken", Bytes::from_static(b"not json")).await?;
    let id = CredentialId::new("broken");
    assert!(matches!(
        store.get_material(REGISTRY, &tenant()?, &id).await,
        Err(CredentialStoreError::Codec(_))
    ));
    assert!(matches!(
        store.rotate_credential(REGISTRY, &tenant()?, &id, &material("v")).await,
        Err(CredentialStoreError::Codec(_))
    ));
    assert!(matches!(
        store.reencrypt_credential(REGISTRY, None, &id).await,
        Err(CredentialStoreError::Codec(_))
    ));
    assert!(matches!(
        store.list_legacy(REGISTRY).await,
        Err(CredentialStoreError::Codec(_))
    ));
    Ok(())
}

async fn seed_legacy(js: &jetstream::Context, id: &str, password: &str) -> Result<(), Box<dyn Error>> {
    let key = kms();
    let (ciphertext, nonce) = key.encrypt(&[], &serde_json::to_vec(&material(password))?).await?;
    let record = serde_json::json!({
        "version": 1,
        "last_rotated": serde_json::to_value(SystemTime::now())?,
        "nonce": nonce,
        "ciphertext": ciphertext,
    });
    let bucket = js.get_key_value(credential_kv_bucket(REGISTRY)).await?;
    bucket.put(id, Bytes::from(serde_json::to_vec(&record)?)).await?;
    Ok(())
}

#[tokio::test]
async fn legacy_records_are_listed_read_and_reencrypted() -> Result<(), Box<dyn Error>> {
    let (_server, store, js) = credential_store().await?;
    store
        .put_credential(REGISTRY, &tenant()?, &CredentialId::new("seed"), &material("v"))
        .await?;
    seed_legacy(&js, "old", "legacy-pass").await?;
    let old = CredentialId::new("old");
    assert_eq!(store.list_legacy(REGISTRY).await?, vec![old.clone()]);
    assert_eq!(
        store.get_material(REGISTRY, &tenant()?, &old).await?,
        material("legacy-pass")
    );
    assert!(matches!(
        store.reencrypt_credential(REGISTRY, None, &old).await,
        Err(CredentialStoreError::LegacyTenantRequired)
    ));
    assert_eq!(store.reencrypt_credential(REGISTRY, Some(&tenant()?), &old).await?, 2);
    assert!(store.list_legacy(REGISTRY).await?.is_empty());
    assert_eq!(
        store.get_material(REGISTRY, &tenant()?, &old).await?,
        material("legacy-pass")
    );
    assert_eq!(store.reencrypt_credential(REGISTRY, None, &old).await?, 3);
    assert!(matches!(
        store
            .reencrypt_credential(REGISTRY, Some(&TenantId::new("other")?), &old)
            .await,
        Err(CredentialStoreError::TenantMismatch)
    ));
    assert!(matches!(
        store
            .reencrypt_credential(REGISTRY, None, &CredentialId::new("missing"))
            .await,
        Err(CredentialStoreError::NotFound)
    ));
    Ok(())
}

#[tokio::test]
async fn legacy_reads_can_be_turned_off() -> Result<(), Box<dyn Error>> {
    let (_server, store, js) = credential_store().await?;
    let store = store.with_legacy_reads(false);
    store
        .put_credential(REGISTRY, &tenant()?, &CredentialId::new("seed"), &material("v"))
        .await?;
    seed_legacy(&js, "old", "p").await?;
    assert!(matches!(
        store
            .get_material(REGISTRY, &tenant()?, &CredentialId::new("old"))
            .await,
        Err(CredentialStoreError::LegacyDisabled)
    ));
    Ok(())
}

#[tokio::test]
async fn a_record_changed_between_read_and_write_is_a_conflict() -> Result<(), Box<dyn Error>> {
    let (_server, store, js) = credential_store().await?;
    let id = CredentialId::new("c1");
    store.put_credential(REGISTRY, &tenant()?, &id, &material("v")).await?;
    let store = Arc::new(store);
    let racers: Vec<_> = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            let id = id.clone();
            tokio::spawn(async move { store.reencrypt_credential(REGISTRY, None, &id).await })
        })
        .collect();
    let mut outcomes = Vec::new();
    for racer in racers {
        outcomes.push(racer.await?);
    }
    assert!(outcomes.iter().any(Result::is_ok));
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, Ok(_) | Err(CredentialStoreError::Conflict)))
    );
    drop(js);
    Ok(())
}

#[tokio::test]
async fn tokens_are_issued_validated_expired_and_revoked() -> Result<(), Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let kv = NatsKvTokenStore::open_bucket(&js).await?;
    let store = NatsKvTokenStore::new(kv.clone());
    let ttl = |seconds| NonZeroU32::new(seconds).ok_or("ttl");
    let issued = store
        .issue(&CredentialId::new("c"), &scope(), ShortLivedToken::new("v"), ttl(300)?)
        .await?;
    assert_eq!(issued.token.expose_secret(), "v");
    let record = store.validate(&issued.token_id).await?;
    assert_eq!(record.credential_id, CredentialId::new("c"));
    store.revoke(&issued.token_id).await?;
    store.revoke(&issued.token_id).await?;
    assert!(matches!(
        store.validate(&issued.token_id).await,
        Err(CredentialError::TokenRevoked { .. })
    ));

    let short = store
        .issue(&CredentialId::new("c"), &scope(), ShortLivedToken::new("v"), ttl(1)?)
        .await?;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(matches!(
        store.validate(&short.token_id).await,
        Err(CredentialError::TokenExpired { .. })
    ));

    let unknown = TokenId::new("nope");
    assert!(matches!(
        store.validate(&unknown).await,
        Err(CredentialError::NotFound { .. })
    ));
    store.revoke(&unknown).await?;

    kv.put("broken", Bytes::from_static(b"not json")).await?;
    let broken = TokenId::new("broken");
    let cut_off = server.client().await;
    let lost = NatsKvTokenStore::new(NatsKvTokenStore::open_bucket(&jetstream::new(cut_off.clone())).await?);
    cut_off.drain().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(matches!(
        lost.validate(&broken).await,
        Err(CredentialError::Backend { .. })
    ));
    assert!(matches!(
        store.validate(&broken).await,
        Err(CredentialError::Backend { .. })
    ));
    assert!(store.revoke(&broken).await.is_err());
    Ok(())
}

#[tokio::test]
async fn committed_policies_are_versioned_per_tenant_and_lease() -> Result<(), Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let kv = NatsKvCommittedPolicies::open_bucket(&js).await?;
    let policies = NatsKvCommittedPolicies::new(kv.clone());
    let lease = LeaseId::new("l1");
    let set = |names: &[&str]| -> HashSet<PullSecretId> { names.iter().map(|name| PullSecretId::new(*name)).collect() };
    assert_eq!(policies.committed(&tenant()?, &lease).await?, None);
    assert_eq!(policies.commit(&tenant()?, &lease, set(&["a"])).await?, 1);
    assert_eq!(policies.commit(&tenant()?, &lease, set(&["a"])).await?, 1);
    assert_eq!(policies.commit(&tenant()?, &lease, set(&["a", "b"])).await?, 2);
    let committed = policies.committed(&tenant()?, &lease).await?.ok_or("committed")?;
    assert_eq!(committed.revision, 2);
    assert_eq!(committed.references, set(&["a", "b"]));
    assert_eq!(policies.committed(&TenantId::new("other")?, &lease).await?, None);

    policies.revoke(&tenant()?, &lease).await?;
    assert_eq!(policies.committed(&tenant()?, &lease).await?, None);
    assert_eq!(policies.commit(&tenant()?, &lease, set(&["a"])).await?, 1);

    let impatient = policies.clone().with_commit_attempts(0);
    assert!(matches!(
        impatient.commit(&tenant()?, &lease, set(&["z"])).await,
        Err(PolicyStoreError::Contended)
    ));

    let key = format!("{}.{}", hex_of("tenant-a"), hex_of("broken"));
    kv.put(key, Bytes::from_static(b"not json")).await?;
    let broken = LeaseId::new("broken");
    assert!(matches!(
        policies.committed(&tenant()?, &broken).await,
        Err(PolicyStoreError::Codec(_))
    ));
    assert!(matches!(
        policies.commit(&tenant()?, &broken, set(&["a"])).await,
        Err(PolicyStoreError::Codec(_))
    ));
    Ok(())
}

fn hex_of(value: &str) -> String {
    value.bytes().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test]
async fn the_audit_stream_records_every_event() -> Result<(), Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let logger = JetStreamAuditLogger::new(js.clone()).await?;
    let again = JetStreamAuditLogger::new(js.clone()).await?;
    logger
        .emit(CredentialAuditEvent::Revoked {
            token_id: TokenId::new("t"),
            reason: "why".to_string(),
            at: SystemTime::now(),
        })
        .await?;
    again
        .emit(CredentialAuditEvent::Revoked {
            token_id: TokenId::new("u"),
            reason: "why".to_string(),
            at: SystemTime::now(),
        })
        .await?;
    let mut stream = js.get_stream(credential_audit_stream_name()).await?;
    assert_eq!(stream.info().await?.state.messages, 2);
    Ok(())
}

#[tokio::test]
async fn a_jetstream_broker_serves_a_full_issue_cycle() -> Result<(), Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let broker = CredentialBroker::jetstream(js, kms(), true, FenceMode::Optional).await?;
    broker
        .store_credential(REGISTRY, &tenant()?, &CredentialId::new("c"), material("pw"))
        .await?;
    let issued = broker
        .issue(
            REGISTRY,
            &tenant()?,
            trogon_sandbox_contract::CredentialIssueRequest {
                pull_secret_id: PullSecretId::new("c"),
                scope: scope(),
                ttl_seconds: NonZeroU32::new(60).ok_or("ttl")?,
                requester: None,
                fence: None,
            },
        )
        .await?;
    assert_eq!(issued.token.expose_secret(), "u:pw");
    broker.validate_token(&issued.token_id).await?;
    Ok(())
}
