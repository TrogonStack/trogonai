use crate::ulid::Ulid;
use async_nats::jetstream::Context;
use bytes::Bytes;
use serde_json::{Value, json};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{NativeSnapshotId, NativeSnapshotRef, ProviderKind, TenantId};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use super::build_id_error::SnapshotBuildIdError;
use super::constants::BUCKET;
use super::*;
use crate::ulid_error::UlidError;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BUILD: &str = "11111111-2222-4333-8444-555555555555";

fn tenant() -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new("tenant-a")?)
}

fn reference(id: &str) -> Result<NativeSnapshotRef, Box<dyn std::error::Error>> {
    Ok(NativeSnapshotRef {
        provider: ProviderKind::E2b,
        id: NativeSnapshotId::new(id)?,
        expires_at: None,
    })
}

fn manifest_json(id: &str, version: u32, executions: Vec<Value>) -> Result<Value, Box<dyn std::error::Error>> {
    let mut manifest = json!({
        "version": version,
        "tenant": tenant()?,
        "reference": reference(id)?,
        "executions": executions,
    });
    if version == 3 {
        manifest["binding"] = json!({ "template_id": "tpl-1", "build_id": BUILD });
        manifest["logical_name"] = json!("checkpoint");
    }
    Ok(manifest)
}

fn execution(id: &str, deadline: Option<Value>) -> Value {
    let mut execution = json!({
        "execution_id": id,
        "user": "user",
        "identity": { "pid": 42, "started": 7 },
        "max_output_bytes": 1024,
    });
    if let Some(deadline) = deadline {
        execution["deadline"] = deadline;
    }
    execution
}

fn manifest(value: Value) -> Result<SnapshotManifest, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(value)?)
}

fn timeout_deadline() -> Value {
    json!({
        "kind": "timeout",
        "issued_unix_ms": 10,
        "expires_unix_ms": 100,
        "captured_unix_ms": 20,
        "remaining_ms": 80,
        "timed_out": false,
    })
}

#[test]
fn canonical_vendor_identity_cannot_be_an_alias_path_or_mutable_tag() -> TestResult {
    for invalid in [
        "",
        ".",
        "..",
        "team/name",
        "raw:default",
        "raw%2fname",
        "raw?query",
        "UPPER",
        "raw\n",
    ] {
        assert!(SnapshotTemplateId::try_from(invalid.to_owned()).is_err());
    }
    assert!(SnapshotTemplateId::try_from("x".repeat(129)).is_err());
    let template = SnapshotTemplateId::try_from("raw-template_1".to_owned())?;
    assert_eq!(template.as_str(), "raw-template_1");
    assert_eq!(String::from(template), "raw-template_1");
    assert!(matches!(
        SnapshotBuildId::try_from("default".to_owned()),
        Err(SnapshotBuildIdError::Malformed)
    ));
    for invalid in [
        "11111111222243338444555555555555",
        "00000000-0000-0000-0000-000000000000",
        "AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE",
    ] {
        assert!(matches!(
            SnapshotBuildId::try_from(invalid.to_owned()),
            Err(SnapshotBuildIdError::NotCanonical)
        ));
    }
    let build = SnapshotBuildId::try_from(BUILD.to_owned())?;
    assert_eq!(String::from(build), BUILD);
    Ok(())
}

#[test]
fn binding_selects_the_exact_build() -> TestResult {
    let binding: SnapshotBinding = serde_json::from_value(json!({ "template_id": "tpl-1", "build_id": BUILD }))?;
    assert_eq!(binding.restore_selector(), format!("tpl-1:{BUILD}"));
    assert!(serde_json::from_value::<SnapshotBinding>(json!({ "template_id": "tpl-1", "build_id": "x" })).is_err());
    Ok(())
}

#[test]
fn logical_names_are_bounded_and_printable() {
    assert!(validate_logical_name("checkpoint").is_ok());
    assert!(validate_logical_name(&"n".repeat(129)).is_err());
    assert!(validate_logical_name("bad\nname").is_err());
}

#[test]
fn opaque_ids_round_trip_and_reject_other_shapes() {
    let id = Ulid::generate().to_string();
    assert_eq!(id.len(), 26);
    assert!(is_opaque_id(&id));
    assert!(!is_opaque_id(&id.to_lowercase()));
    assert!(!is_opaque_id(&uuid::Uuid::now_v7().simple().to_string()));
    assert!(!is_opaque_id("01ARZ3NDEKTSV4RRFFQ69G5FA"));
}

#[test]
fn ulids_parse_the_reference_value_in_either_case() -> TestResult {
    let reference = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    assert_eq!(Ulid::parse(reference)?.to_string(), reference);
    assert_eq!(Ulid::parse(&reference.to_lowercase())?.to_string(), reference);
    assert_eq!(Ulid::parse("01ARZ3NDEKTSV4RRFFQ69G5FA").err(), Some(UlidError::Length));
    assert_eq!(
        Ulid::parse("01ARZ3NDEKTSV4RRFFQ69G5FAU").err(),
        Some(UlidError::InvalidCharacter)
    );
    assert_eq!(
        Ulid::parse("81ARZ3NDEKTSV4RRFFQ69G5FAV").err(),
        Some(UlidError::Overflow)
    );
    Ok(())
}

#[test]
fn process_identity_parses_the_launcher_output() -> TestResult {
    let identity = ProcessIdentity::parse(b"42 7\n")?;
    assert_eq!((identity.pid, identity.started), (42, 7));
    assert!(identity.is_real());
    assert!(matches!(
        ProcessIdentity::parse(&[b'1'; 129]),
        Err(super::process_identity_error::ProcessIdentityError::TooLong)
    ));
    assert!(matches!(
        ProcessIdentity::parse(&[0xff]),
        Err(super::process_identity_error::ProcessIdentityError::NotUtf8(_))
    ));
    assert!(matches!(
        ProcessIdentity::parse(b""),
        Err(super::process_identity_error::ProcessIdentityError::MissingPid)
    ));
    assert!(matches!(
        ProcessIdentity::parse(b"42"),
        Err(super::process_identity_error::ProcessIdentityError::MissingStart)
    ));
    assert!(matches!(
        ProcessIdentity::parse(b"x 7"),
        Err(super::process_identity_error::ProcessIdentityError::NotNumeric(_))
    ));
    for invalid in [&b"42 7 9"[..], b"1 7", b"42 0"] {
        assert!(matches!(
            ProcessIdentity::parse(invalid),
            Err(super::process_identity_error::ProcessIdentityError::Invalid)
        ));
    }
    Ok(())
}

#[test]
fn captured_executions_name_their_control_directory() -> TestResult {
    let captured: CapturedExecution = serde_json::from_value(execution("abc", None))?;
    assert_eq!(captured.directory(), "/tmp/.trogon-exec-abc");
    Ok(())
}

#[test]
fn historical_manifests_remain_readable_without_inventing_capture_authority() -> TestResult {
    let tenant = tenant()?;
    let reference = reference("historical-id")?;
    for version in [1, 2] {
        let manifest = manifest(manifest_json("historical-id", version, vec![])?)?;
        manifest.validate(&tenant, &reference)?;
        assert!(matches!(
            manifest.restore_binding(),
            Err(SnapshotManifestError::LegacyRestore)
        ));
    }
    Ok(())
}

#[test]
fn current_manifests_expose_their_binding() -> TestResult {
    let id = Ulid::generate().to_string();
    let reference = reference(&id)?;
    let current = manifest(manifest_json(&id, 3, vec![])?)?;
    current.validate(&tenant()?, &reference)?;
    assert_eq!(current.restore_binding()?.template_id.as_str(), "tpl-1");
    let mut unbound = manifest(manifest_json(&id, 3, vec![])?)?;
    unbound.binding = None;
    assert!(matches!(
        unbound.restore_binding(),
        Err(SnapshotManifestError::MissingBinding)
    ));
    Ok(())
}

#[test]
fn manifest_validation_rejects_every_inconsistency() -> TestResult {
    let tenant = tenant()?;
    let id = Ulid::generate().to_string();
    let reference = reference(&id)?;
    let check = |value: Value| -> Result<SnapshotManifestError, Box<dyn std::error::Error>> {
        let error = manifest(value)?
            .validate(&tenant, &reference)
            .err()
            .ok_or("manifest was accepted")?;
        Ok(error)
    };

    let mut value = manifest_json(&id, 3, vec![])?;
    value["version"] = json!(9);
    assert!(matches!(check(value)?, SnapshotManifestError::OwnerOrVersionMismatch));

    let mut value = manifest_json(&id, 3, vec![])?;
    value["tenant"] = json!("tenant-b");
    assert!(matches!(check(value)?, SnapshotManifestError::OwnerOrVersionMismatch));

    let mut value = manifest_json(&id, 3, vec![])?;
    value["reference"]["id"] = json!("other-id");
    assert!(matches!(check(value)?, SnapshotManifestError::IdentityMismatch));

    let mut value = manifest_json(&id, 3, vec![])?;
    value["reference"]["expires_at"] = json!(1);
    assert!(matches!(check(value)?, SnapshotManifestError::Reference(_)));

    let too_many: Vec<Value> = (0..=MAX_EXECUTIONS)
        .map(|_| execution(&Ulid::generate().to_string(), Some(json!({ "kind": "lease" }))))
        .collect();
    assert!(matches!(
        check(manifest_json(&id, 3, too_many)?)?,
        SnapshotManifestError::TooManyExecutions
    ));

    let mut value = manifest_json(&id, 3, vec![])?;
    value.as_object_mut().ok_or("not an object")?.remove("binding");
    assert!(matches!(check(value)?, SnapshotManifestError::MissingBinding));

    let legacy_id = "legacy-id";
    let legacy_reference = self::reference(legacy_id)?;
    let mut value = manifest_json(legacy_id, 3, vec![])?;
    value["reference"] = json!(legacy_reference);
    let error = manifest(value)?
        .validate(&tenant, &legacy_reference)
        .err()
        .ok_or("manifest was accepted")?;
    assert!(matches!(error, SnapshotManifestError::InvalidReferenceId));

    let mut value = manifest_json(&id, 3, vec![])?;
    value.as_object_mut().ok_or("not an object")?.remove("logical_name");
    assert!(matches!(check(value)?, SnapshotManifestError::MissingLogicalName));

    let mut value = manifest_json(&id, 3, vec![])?;
    value["logical_name"] = json!("bad\nname");
    assert!(matches!(check(value)?, SnapshotManifestError::LogicalName(_)));

    for version in [1, 2] {
        let mut value = manifest_json(&id, version, vec![])?;
        value["logical_name"] = json!("checkpoint");
        let legacy = manifest(value)?;
        let error = legacy
            .validate_identity(&tenant, &reference)
            .err()
            .ok_or("manifest was accepted")?;
        assert!(matches!(error, SnapshotManifestError::LegacyBindingFields));
    }
    Ok(())
}

#[test]
fn captured_executions_must_match_their_manifest_version() -> TestResult {
    let tenant = tenant()?;
    let id = Ulid::generate().to_string();
    let reference = reference(&id)?;
    let exec_id = Ulid::generate().to_string();
    let lease = || Some(json!({ "kind": "lease" }));
    let check = |version: u32, executions: Vec<Value>| -> Result<SnapshotManifestError, Box<dyn std::error::Error>> {
        manifest(manifest_json(&id, version, executions)?)?
            .validate(&tenant, &reference)
            .err()
            .ok_or_else(|| "manifest was accepted".into())
    };

    assert!(matches!(
        check(1, vec![execution(&exec_id, lease())])?,
        SnapshotManifestError::DeadlineVersionMismatch
    ));
    assert!(matches!(
        check(2, vec![execution(&exec_id, None)])?,
        SnapshotManifestError::DeadlineVersionMismatch
    ));
    let bad_deadline = json!({
        "kind": "timeout", "issued_unix_ms": 0, "expires_unix_ms": 100,
        "captured_unix_ms": 20, "remaining_ms": 80, "timed_out": false,
    });
    assert!(matches!(
        check(3, vec![execution(&exec_id, Some(bad_deadline))])?,
        SnapshotManifestError::Deadline(_)
    ));
    assert!(matches!(
        check(3, vec![execution("not-opaque", lease())])?,
        SnapshotManifestError::InvalidExecutionId
    ));
    assert!(matches!(
        check(3, vec![execution(&exec_id, lease()), execution(&exec_id, lease())])?,
        SnapshotManifestError::DuplicateExecutionId
    ));
    let mut unreal = execution(&exec_id, lease());
    unreal["identity"]["pid"] = json!(1);
    assert!(matches!(
        check(3, vec![unreal])?,
        SnapshotManifestError::InvalidProcessIdentity
    ));

    let terminal = Some(json!({ "kind": "terminal", "exit_code": 3 }));
    for deadline in [lease(), Some(timeout_deadline()), terminal] {
        manifest(manifest_json(&id, 3, vec![execution(&exec_id, deadline)])?)?.validate(&tenant, &reference)?;
    }
    Ok(())
}

#[test]
fn deadline_provenance_must_be_consistent() -> TestResult {
    for (issued, expires, captured, remaining) in
        [(0, 100, 20, 80), (10, 10, 20, 0), (10, 100, 5, 80), (10, 100, 20, 81)]
    {
        let deadline: CapturedDeadline = serde_json::from_value(json!({
            "kind": "timeout", "issued_unix_ms": issued, "expires_unix_ms": expires,
            "captured_unix_ms": captured, "remaining_ms": remaining, "timed_out": false,
        }))?;
        assert!(deadline.validate().is_err());
    }
    Ok(())
}

#[test]
fn references_must_name_a_live_e2b_snapshot() -> TestResult {
    let live = reference("snapshot-1")?;
    validate_reference(&live)?;
    validate_reference_identity(&live)?;

    let mut other = reference("snapshot-1")?;
    other.provider = ProviderKind::Vercel;
    assert!(matches!(
        validate_reference_identity(&other),
        Err(SnapshotReferenceError::NotE2b)
    ));
    assert!(matches!(
        validate_reference_identity(&reference("..")?),
        Err(SnapshotReferenceError::NotE2b)
    ));

    let mut expired = reference("snapshot-1")?;
    expired.expires_at = Some(1);
    assert!(matches!(
        validate_reference(&expired),
        Err(SnapshotReferenceError::Expired)
    ));
    validate_reference_identity(&expired)?;
    Ok(())
}

async fn configured() -> Result<(JetStreamTestServer, Context, SnapshotManifests), Box<dyn std::error::Error>> {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let manifests = SnapshotManifests::default();
    manifests.configure(context.clone())?;
    Ok((server, context, manifests))
}

fn current_manifest(id: &str) -> Result<SnapshotManifest, Box<dyn std::error::Error>> {
    manifest(manifest_json(id, 3, vec![])?)
}

#[tokio::test]
async fn manifests_require_a_context_configured_once() -> TestResult {
    let manifests = SnapshotManifests::default();
    assert!(matches!(
        manifests.ready().await,
        Err(SnapshotManifestsError::NotConfigured)
    ));
    let server = JetStreamTestServer::start().await;
    manifests.configure(server.jetstream().await)?;
    assert!(matches!(
        manifests.configure(server.jetstream().await),
        Err(SnapshotManifestsError::AlreadyConfigured)
    ));
    manifests.ready().await?;
    Ok(())
}

#[tokio::test]
async fn manifests_are_recorded_once_loaded_and_removed() -> TestResult {
    let (_server, _context, manifests) = configured().await?;
    let tenant = tenant()?;
    let id = Ulid::generate().to_string();
    let reference = reference(&id)?;
    assert!(manifests.load(&tenant, &reference).await?.is_none());

    let recorded = current_manifest(&id)?;
    assert_eq!(manifests.record(&recorded).await?, RecordOutcome::Created);
    assert_eq!(manifests.record(&recorded).await?, RecordOutcome::AlreadyExists);
    let loaded = manifests
        .load(&tenant, &reference)
        .await?
        .ok_or("manifest is missing")?;
    assert_eq!(loaded.restore_binding()?.template_id.as_str(), "tpl-1");

    manifests.remove(&tenant, &reference).await?;
    assert!(manifests.load_for_delete(&tenant, &reference).await?.is_none());

    let mut expired = self::reference(&id)?;
    expired.expires_at = Some(1);
    assert!(matches!(
        manifests.load(&tenant, &expired).await,
        Err(SnapshotManifestsError::Reference(SnapshotReferenceError::Expired))
    ));
    Ok(())
}

#[tokio::test]
async fn manifests_refuse_invalid_or_oversized_records() -> TestResult {
    let (_server, _context, manifests) = configured().await?;
    let id = Ulid::generate().to_string();

    let mut invalid = current_manifest(&id)?;
    invalid.version = 9;
    assert!(matches!(
        manifests.record(&invalid).await,
        Err(SnapshotManifestsError::Manifest(_))
    ));

    let executions: Vec<Value> = (0..MAX_EXECUTIONS)
        .map(|_| {
            let mut execution = execution(&Ulid::generate().to_string(), Some(timeout_deadline()));
            execution["user"] = json!("u".repeat(32));
            execution["max_output_bytes"] = json!(u64::MAX);
            execution["identity"]["started"] = json!(u64::MAX);
            execution
        })
        .collect();
    let oversized = manifest(manifest_json(&id, 3, executions)?)?;
    assert!(matches!(
        manifests.record(&oversized).await,
        Err(SnapshotManifestsError::TooLarge)
    ));
    Ok(())
}

#[tokio::test]
async fn stored_manifests_are_checked_when_read() -> TestResult {
    let (_server, context, manifests) = configured().await?;
    let tenant = tenant()?;
    let id = Ulid::generate().to_string();
    let reference = reference(&id)?;
    manifests.ready().await?;
    let bucket = context.get_key_value(BUCKET).await?;
    let key = tenant_scoped_kv_key(&tenant, &id);

    bucket.put(&key, Bytes::from_static(b"not json")).await?;
    assert!(matches!(
        manifests.load_for_delete(&tenant, &reference).await,
        Err(SnapshotManifestsError::Decode(_))
    ));

    bucket.put(&key, Bytes::from(vec![b' '; 61 * 1024])).await?;
    assert!(matches!(
        manifests.load_for_delete(&tenant, &reference).await,
        Err(SnapshotManifestsError::TooLarge)
    ));

    let mut foreign = manifest_json(&id, 3, vec![])?;
    foreign["tenant"] = json!("tenant-b");
    bucket.put(&key, Bytes::from(serde_json::to_vec(&foreign)?)).await?;
    assert!(matches!(
        manifests.load_for_delete(&tenant, &reference).await,
        Err(SnapshotManifestsError::Manifest(_))
    ));
    Ok(())
}

#[tokio::test]
async fn manifest_storage_failures_are_reported() -> TestResult {
    let (_server, context, manifests) = configured().await?;
    let tenant = tenant()?;
    let id = Ulid::generate().to_string();
    let reference = reference(&id)?;
    manifests.ready().await?;
    context.delete_key_value(BUCKET).await?;

    assert!(matches!(
        manifests.record(&current_manifest(&id)?).await,
        Err(SnapshotManifestsError::Bucket(_))
    ));
    assert!(matches!(
        manifests.load_for_delete(&tenant, &reference).await,
        Err(SnapshotManifestsError::Bucket(_))
    ));
    assert!(matches!(
        manifests.remove(&tenant, &reference).await,
        Err(SnapshotManifestsError::Bucket(_))
    ));
    Ok(())
}

#[tokio::test]
async fn an_expiring_bucket_is_refused_on_open_and_after_a_change() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    context
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: BUCKET.into(),
            history: 1,
            max_age: std::time::Duration::from_secs(600),
            ..Default::default()
        })
        .await?;
    let manifests = SnapshotManifests::default();
    manifests.configure(context)?;
    assert!(matches!(
        manifests.ready().await,
        Err(SnapshotManifestsError::Bucket(crate::CustodyBucketError::NotDurable))
    ));

    let (_server, context, manifests) = configured().await?;
    manifests.ready().await?;
    let mut config = context
        .get_stream(format!("KV_{BUCKET}"))
        .await?
        .cached_info()
        .config
        .clone();
    config.max_age = std::time::Duration::from_secs(600);
    context.update_stream(config).await?;
    assert!(matches!(
        manifests.ready().await,
        Err(SnapshotManifestsError::Bucket(crate::CustodyBucketError::NotDurable))
    ));
    Ok(())
}
