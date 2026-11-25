use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    CloneAuth, Digest, EmptyObject, HostMountAccess, SecretRef, VolumeMode, WorkspaceSource,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const SECRET: &str = "hunter2-literal";

fn round_trip(source: &WorkspaceSource, wire: serde_json::Value) -> TestResult {
    assert_eq!(serde_json::to_value(source)?, wire);
    assert_eq!(&serde_json::from_value::<WorkspaceSource>(wire)?, source);
    Ok(())
}

#[test]
fn empty_workspaces_serialize_as_an_empty_variant_object() -> TestResult {
    round_trip(&WorkspaceSource::empty(), json!({ "empty": {} }))?;
    let error = serde_json::from_value::<WorkspaceSource>(json!({ "empty": { "extra": 1 } }))
        .err()
        .ok_or("empty workspace with fields deserialized")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn clone_workspaces_carry_their_auth_under_the_variant_key() -> TestResult {
    let source = WorkspaceSource::Clone {
        url: "https://example.com/repo.git".to_owned(),
        sha: "abc123".to_owned(),
        base_sha: None,
        auth: CloneAuth::BearerToken {
            token_ref: SecretRef::Literal {
                value: SECRET.to_owned(),
            },
        },
    };
    round_trip(
        &source,
        json!({ "clone": {
            "url": "https://example.com/repo.git",
            "sha": "abc123",
            "base_sha": null,
            "auth": { "bearer_token": { "token_ref": { "literal": { "value": SECRET } } } },
        } }),
    )?;

    let parsed: WorkspaceSource = serde_json::from_value(json!({ "clone": {
        "url": "https://example.com/repo.git",
        "sha": "abc123",
        "auth": { "none": {} },
    } }))?;
    assert_eq!(
        parsed,
        WorkspaceSource::Clone {
            url: "https://example.com/repo.git".to_owned(),
            sha: "abc123".to_owned(),
            base_sha: None,
            auth: CloneAuth::None(EmptyObject::default()),
        }
    );
    Ok(())
}

#[test]
fn clone_auth_variants_use_snake_case_keys() -> TestResult {
    let none = CloneAuth::None(EmptyObject::default());
    assert_eq!(serde_json::to_value(&none)?, json!({ "none": {} }));
    assert_eq!(serde_json::from_value::<CloneAuth>(json!({ "none": {} }))?, none);

    let basic = CloneAuth::Basic {
        username: "deploy".to_owned(),
        password_ref: SecretRef::NatsKv {
            bucket: "secrets".to_owned(),
            key: "repo.password".to_owned(),
        },
    };
    let wire = json!({ "basic": {
        "username": "deploy",
        "password_ref": { "nats_kv": { "bucket": "secrets", "key": "repo.password" } },
    } });
    assert_eq!(serde_json::to_value(&basic)?, wire);
    assert_eq!(serde_json::from_value::<CloneAuth>(wire)?, basic);
    Ok(())
}

#[test]
fn host_mounts_read_readonly_as_their_access() -> TestResult {
    let read_only = WorkspaceSource::HostMount {
        path: "/srv/workspaces/acme".to_owned(),
        access: HostMountAccess::ReadOnly,
    };
    round_trip(
        &read_only,
        json!({ "host_mount": { "path": "/srv/workspaces/acme", "readonly": true } }),
    )?;

    let defaulted: WorkspaceSource = serde_json::from_value(json!({ "host_mount": { "path": "/srv/ws" } }))?;
    assert_eq!(
        defaulted,
        WorkspaceSource::HostMount {
            path: "/srv/ws".to_owned(),
            access: HostMountAccess::ReadWrite,
        }
    );
    assert_eq!(
        serde_json::to_value(&defaulted)?,
        json!({ "host_mount": { "path": "/srv/ws", "readonly": false } })
    );
    Ok(())
}

#[test]
fn host_mount_access_maps_to_the_readonly_flag() {
    assert_eq!(HostMountAccess::default(), HostMountAccess::ReadWrite);
    assert!(HostMountAccess::ReadOnly.is_read_only());
    assert!(!HostMountAccess::ReadWrite.is_read_only());
    assert_eq!(HostMountAccess::from(true), HostMountAccess::ReadOnly);
    assert_eq!(HostMountAccess::from(false), HostMountAccess::ReadWrite);
    assert!(bool::from(HostMountAccess::ReadOnly));
    assert!(!bool::from(HostMountAccess::ReadWrite));
}

#[test]
fn s3_volumes_omit_an_absent_snapshot_and_reject_invalid_digests() -> TestResult {
    let seeded = WorkspaceSource::S3Volume {
        bucket: "workspaces".to_owned(),
        prefix: "acme/".to_owned(),
        mode: VolumeMode::CopyOnWrite,
        snapshot: Some(Digest::new(DIGEST)?),
    };
    round_trip(
        &seeded,
        json!({ "s3_volume": {
            "bucket": "workspaces",
            "prefix": "acme/",
            "mode": "copy_on_write",
            "snapshot": DIGEST,
        } }),
    )?;

    let fresh = WorkspaceSource::S3Volume {
        bucket: "workspaces".to_owned(),
        prefix: "acme/".to_owned(),
        mode: VolumeMode::ReadOnly,
        snapshot: None,
    };
    round_trip(
        &fresh,
        json!({ "s3_volume": { "bucket": "workspaces", "prefix": "acme/", "mode": "read_only" } }),
    )?;

    let error = serde_json::from_value::<WorkspaceSource>(json!({ "s3_volume": {
        "bucket": "workspaces",
        "prefix": "acme/",
        "mode": "read_only",
        "snapshot": "sha256:short",
    } }))
    .err()
    .ok_or("invalid snapshot digest deserialized")?;
    assert_eq!(error.classify(), Data);
    assert!(serde_json::from_value::<WorkspaceSource>(json!({ "git": {} })).is_err());
    Ok(())
}

#[test]
fn secret_refs_never_print_literal_values() -> TestResult {
    let literal = SecretRef::Literal {
        value: SECRET.to_owned(),
    };
    assert_eq!(
        serde_json::to_value(&literal)?,
        json!({ "literal": { "value": SECRET } })
    );
    assert_eq!(format!("{literal}"), "[REDACTED]");
    assert_eq!(format!("{literal:?}"), r#"Literal { value: "[REDACTED]" }"#);

    let auth = CloneAuth::BearerToken { token_ref: literal };
    assert_eq!(
        format!("{auth:?}"),
        r#"BearerToken { token_ref: Literal { value: "[REDACTED]" } }"#
    );

    let stored = SecretRef::NatsKv {
        bucket: "secrets".to_owned(),
        key: "token".to_owned(),
    };
    assert_eq!(format!("{stored}"), "nats-kv:secrets/token");
    assert_eq!(format!("{stored:?}"), r#"NatsKv { bucket: "secrets", key: "token" }"#);
    assert_eq!(
        serde_json::from_value::<SecretRef>(json!({ "nats_kv": { "bucket": "secrets", "key": "token" } }))?,
        stored
    );
    Ok(())
}
