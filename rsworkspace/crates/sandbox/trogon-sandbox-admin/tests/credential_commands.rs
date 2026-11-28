//! Runs the `credential` commands against a private NATS server: set, list, rotate, migrate and
//! revoke-token, through the library and through the binary.

use std::error::Error;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_admin::credential::{self, MigrateRequest};
use trogon_sandbox_admin::{AdminError, Console};
use trogon_sandbox_contract::{CredentialId, CredentialProvider, CredentialScope, ShortLivedToken, TenantId, TokenId};
use trogon_sandbox_credential_broker::{
    BrokerService, CredentialBroker, CredentialMaterial, CredentialStore, InMemoryAuditLogger, InMemoryCredentialStore,
    InMemoryTokenStore, InProcessKey, KmsProvider, NatsKvCredentialStore, TokenStore,
};
use trogon_sandbox_protocol::subjects::credential_kv_bucket;

type TestResult = Result<(), Box<dyn Error>>;

const REGISTRY: CredentialProvider = CredentialProvider::Registry;
/// Base64 of 32 bytes of 0x41, the form `--kms-key` takes.
const KMS_KEY_BASE64: &str = "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=";
const BIN: &str = env!("CARGO_BIN_EXE_trogon-sandbox-admin");

#[derive(Default)]
struct Buffers {
    out: Vec<u8>,
    err: Vec<u8>,
}

impl Buffers {
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.out).into_owned()
    }

    fn err(&self) -> String {
        String::from_utf8_lossy(&self.err).into_owned()
    }
}

fn key() -> InProcessKey {
    InProcessKey::from_bytes([0x41; 32])
}

fn tenant() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("acme")?)
}

fn material_json(password: &str) -> String {
    format!(r#"{{"kind":"registry_basic","username":"u","password":"{password}"}}"#)
}

fn material(password: &str) -> CredentialMaterial {
    CredentialMaterial::RegistryBasic {
        username: "u".to_string(),
        password: password.to_string(),
    }
}

async fn set(js: &jetstream::Context, id: &str, password: &str) -> TestResult {
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    credential::set(
        js.clone(),
        key(),
        REGISTRY,
        &tenant()?,
        &CredentialId::new(id),
        &material_json(password),
        &mut console,
    )
    .await?;
    Ok(())
}

/// Writes a record the way the format before envelopes did: sealed with no associated data.
async fn seed_legacy(js: &jetstream::Context, id: &str, password: &str) -> TestResult {
    let (ciphertext, nonce) = key().encrypt(&[], &serde_json::to_vec(&material(password))?).await?;
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

async fn migrate(js: &jetstream::Context, request: MigrateRequest) -> (Result<(), AdminError>, Buffers) {
    let mut buffers = Buffers::default();
    let result = {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        credential::migrate(js.clone(), key(), request, &mut console).await
    };
    (result, buffers)
}

#[tokio::test]
async fn set_seals_a_credential_and_list_shows_it() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let mut empty = Buffers::default();
    {
        let mut console = Console::new(&mut empty.out, &mut empty.err);
        credential::list(js.clone(), key(), REGISTRY, &mut console).await?;
    }
    assert!(
        empty.out().contains("no credentials for provider registry"),
        "{}",
        empty.out()
    );

    let mut stored = Buffers::default();
    {
        let mut console = Console::new(&mut stored.out, &mut stored.err);
        credential::set(
            js.clone(),
            key(),
            REGISTRY,
            &tenant()?,
            &CredentialId::new("pull"),
            &material_json("hunter2"),
            &mut console,
        )
        .await?;
    }
    assert!(stored.err().contains("stored credential pull"), "{}", stored.err());
    assert!(!stored.out().contains("hunter2") && !stored.err().contains("hunter2"));

    let mut listed = Buffers::default();
    {
        let mut console = Console::new(&mut listed.out, &mut listed.err);
        credential::list(js.clone(), key(), REGISTRY, &mut console).await?;
    }
    assert!(listed.out().contains("pull  last_rotated="), "{}", listed.out());
    assert!(listed.out().contains("version=1"), "{}", listed.out());
    assert!(!listed.out().contains("hunter2"));

    let store = NatsKvCredentialStore::new(js, Arc::new(key()));
    assert_eq!(
        store
            .get_material(REGISTRY, &tenant()?, &CredentialId::new("pull"))
            .await?,
        material("hunter2")
    );
    Ok(())
}

#[tokio::test]
async fn set_rejects_material_that_is_not_credential_json() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    let result = credential::set(
        js,
        key(),
        REGISTRY,
        &tenant()?,
        &CredentialId::new("pull"),
        "not json",
        &mut console,
    )
    .await;
    assert!(matches!(result, Err(AdminError::Parse { .. })));
    Ok(())
}

#[tokio::test]
async fn rotate_bumps_the_version_and_keeps_the_material() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    set(&js, "pull", "hunter2").await?;

    let mut buffers = Buffers::default();
    {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        credential::rotate(
            js.clone(),
            key(),
            REGISTRY,
            &tenant()?,
            &CredentialId::new("pull"),
            &mut console,
        )
        .await?;
    }
    assert!(buffers.out().contains("rotated pull to version 2"), "{}", buffers.out());

    let store = NatsKvCredentialStore::new(js.clone(), Arc::new(key()));
    assert_eq!(
        store
            .get_material(REGISTRY, &tenant()?, &CredentialId::new("pull"))
            .await?,
        material("hunter2")
    );

    let mut missing = Buffers::default();
    let mut console = Console::new(&mut missing.out, &mut missing.err);
    let result = credential::rotate(
        js,
        key(),
        REGISTRY,
        &tenant()?,
        &CredentialId::new("absent"),
        &mut console,
    )
    .await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn migrate_dry_run_reports_without_writing_then_a_real_run_rewrites() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    set(&js, "current", "new").await?;
    seed_legacy(&js, "old", "legacy-pass").await?;

    let request = |dry_run| -> Result<MigrateRequest, Box<dyn Error>> {
        Ok(MigrateRequest {
            tenant: tenant()?,
            provider: Some(REGISTRY),
            dry_run,
            only: Vec::new(),
        })
    };

    let (dry, dry_buffers) = migrate(&js, request(true)?).await;
    dry?;
    let out = dry_buffers.out();
    assert!(out.contains("would migrate registry old"), "{out}");
    assert!(out.contains("legacy remaining 1 (dry run)"), "{out}");

    let (real, real_buffers) = migrate(&js, request(false)?).await;
    real?;
    let out = real_buffers.out();
    assert!(out.contains("migrated registry old"), "{out}");
    assert!(
        out.contains("migrated 1, would migrate 0, failed 0, legacy remaining 0"),
        "{out}"
    );

    let strict = NatsKvCredentialStore::new(js.clone(), Arc::new(key())).with_legacy_reads(false);
    assert_eq!(
        strict
            .get_material(REGISTRY, &tenant()?, &CredentialId::new("old"))
            .await?,
        material("legacy-pass")
    );

    let (again, again_buffers) = migrate(&js, request(false)?).await;
    again?;
    assert!(again_buffers.out().contains("migrated 0"), "{}", again_buffers.out());
    Ok(())
}

#[tokio::test]
async fn migrate_with_named_ids_leaves_the_others_and_every_provider_is_visited() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    set(&js, "current", "new").await?;
    seed_legacy(&js, "first", "a").await?;
    seed_legacy(&js, "second", "b").await?;

    let (result, buffers) = migrate(
        &js,
        MigrateRequest {
            tenant: tenant()?,
            provider: None,
            dry_run: false,
            only: vec![CredentialId::new("first")],
        },
    )
    .await;
    result?;
    let out = buffers.out();
    assert!(out.contains("migrated registry first"), "{out}");
    assert!(!out.contains("migrated registry second"), "{out}");
    assert!(
        out.contains("migrated 1, would migrate 0, failed 0, legacy remaining 1"),
        "{out}"
    );
    Ok(())
}

#[tokio::test]
async fn migrate_fails_on_a_record_that_cannot_be_read() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    set(&js, "current", "new").await?;
    let bucket = js.get_key_value(credential_kv_bucket(REGISTRY)).await?;
    bucket.put("broken", Bytes::from_static(b"not json")).await?;

    let (result, _) = migrate(
        &js,
        MigrateRequest {
            tenant: tenant()?,
            provider: Some(REGISTRY),
            dry_run: false,
            only: Vec::new(),
        },
    )
    .await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn revoke_token_invalidates_a_token_through_the_broker() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;

    let credentials = Arc::new(InMemoryCredentialStore::new(Arc::new(key())));
    let tokens = Arc::new(InMemoryTokenStore::new());
    let broker = Arc::new(CredentialBroker::new(
        credentials,
        tokens.clone(),
        Arc::new(InMemoryAuditLogger::new()),
    ));
    let issued = tokens
        .issue(
            &CredentialId::new("pull"),
            &CredentialScope::RegistryPull {
                repository_glob: "r/*".to_string(),
            },
            ShortLivedToken::new("secret-token"),
            std::num::NonZeroU32::new(300).ok_or("zero ttl")?,
        )
        .await?;
    broker.validate_token(&issued.token_id).await?;

    let subscriptions = Arc::new(BrokerService::new(broker.clone()))
        .subscribe(client.clone())
        .await?;
    let serving = tokio::spawn(subscriptions.run(std::future::pending()));

    let mut buffers = Buffers::default();
    {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        credential::revoke_token(
            client.clone(),
            REGISTRY,
            tenant()?,
            issued.token_id.clone(),
            &mut console,
        )
        .await?;
    }
    assert!(
        buffers.out().contains(&format!("revoked token {}", issued.token_id)),
        "{}",
        buffers.out()
    );
    assert!(broker.validate_token(&issued.token_id).await.is_err());
    serving.abort();
    Ok(())
}

#[tokio::test]
async fn revoke_token_without_a_broker_fails() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        credential::revoke_token(client, REGISTRY, tenant()?, TokenId::new("tok"), &mut console),
    )
    .await?;
    assert!(result.is_err());
    Ok(())
}

async fn binary(url: &str, args: &[&str], stdin: Option<&str>) -> Result<std::process::Output, Box<dyn Error>> {
    let mut child = Command::new(BIN)
        .args(["--nats-url", url, "--kms-key", KMS_KEY_BASE64])
        .args(args)
        .env_remove("NATS_URL")
        .env_remove("SANDBOX_KMS_KEY")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes()).await?;
    }
    Ok(child.wait_with_output().await?)
}

#[tokio::test]
async fn the_binary_sets_lists_rotates_and_migrates_credentials() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let url = server.address();

    let stored = binary(
        url,
        &["credential", "set", "--tenant", "acme", "registry", "pull"],
        Some(&material_json("hunter2")),
    )
    .await?;
    assert!(stored.status.success(), "{}", String::from_utf8_lossy(&stored.stderr));
    assert!(String::from_utf8_lossy(&stored.stderr).contains("stored credential pull"));

    let listed = binary(url, &["credential", "list", "registry"], None).await?;
    assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
    assert!(String::from_utf8_lossy(&listed.stdout).contains("pull  last_rotated="));

    let rotated = binary(
        url,
        &["credential", "rotate", "--tenant", "acme", "registry", "pull"],
        None,
    )
    .await?;
    assert!(rotated.status.success(), "{}", String::from_utf8_lossy(&rotated.stderr));
    assert!(String::from_utf8_lossy(&rotated.stdout).contains("rotated pull to version 2"));

    let js = server.jetstream().await;
    seed_legacy(&js, "old", "legacy-pass").await?;
    let dry = binary(
        url,
        &["credential", "migrate", "--tenant", "acme", "--dry-run", "registry"],
        None,
    )
    .await?;
    assert!(dry.status.success(), "{}", String::from_utf8_lossy(&dry.stderr));
    assert!(String::from_utf8_lossy(&dry.stdout).contains("would migrate registry old"));

    let real = binary(
        url,
        &[
            "credential",
            "migrate",
            "--tenant",
            "acme",
            "--credential-id",
            "old",
            "registry",
        ],
        None,
    )
    .await?;
    assert!(real.status.success(), "{}", String::from_utf8_lossy(&real.stderr));
    assert!(String::from_utf8_lossy(&real.stdout).contains("migrated registry old"));
    Ok(())
}

#[tokio::test]
async fn the_binary_revokes_a_token_and_reports_a_missing_broker() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let output = binary(
        server.address(),
        &["credential", "revoke-token", "--tenant", "acme", "registry", "tok"],
        None,
    )
    .await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("revoke token"));
    Ok(())
}
