use std::str::FromStr;

use serde_json::json;
use trogon_sandbox_contract::{
    ArtifactType, Digest, NativeSnapshotId, NativeSnapshotIdError, NativeSnapshotReceipt, NativeSnapshotRef,
    OciArtifactRef, OciImageRef, ProviderKind, PullSecretId, ServiceAccountId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn digest(hex: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(format!("sha256:{}", hex.to_string().repeat(64)))?)
}

#[test]
fn oci_image_ref_splits_registry_repository_tag_and_digest() -> TestResult {
    let pinned = digest('a')?;
    let cases = [
        ("ubuntu:24.04", "", "ubuntu", Some("24.04"), None),
        ("library/redis", "", "library/redis", None, None),
        ("localhost/app", "localhost", "app", None, None),
        ("localhost:5000/app:dev", "localhost:5000", "app", Some("dev"), None),
        (
            "registry.example.com:5000/app",
            "registry.example.com:5000",
            "app",
            None,
            None,
        ),
        ("app:", "", "app:", None, None),
        ("app@latest", "", "app@latest", None, None),
        ("app@sha256:BAD", "", "app@sha256", Some("BAD"), None),
    ];
    for (input, registry, repository, tag, expected_digest) in cases {
        let image = OciImageRef::parse_unqualified(input);
        assert_eq!(image.registry, registry, "{input}");
        assert_eq!(image.repository, repository, "{input}");
        assert_eq!(image.tag.as_deref(), tag, "{input}");
        assert_eq!(image.digest, expected_digest, "{input}");
    }

    let input = format!("ghcr.io/team-a/app:1.0@{pinned}");
    let image = OciImageRef::parse_unqualified(&input);
    assert_eq!(
        image,
        OciImageRef {
            registry: "ghcr.io".to_owned(),
            repository: "team-a/app".to_owned(),
            tag: Some("1.0".to_owned()),
            digest: Some(pinned),
        }
    );
    assert_eq!(image.allowlist_string(), input);
    assert_eq!(image.to_string(), input);
    Ok(())
}

#[test]
fn oci_image_ref_allowlist_string_omits_absent_parts() {
    let image = OciImageRef::parse_unqualified("ubuntu");
    assert_eq!(image.allowlist_string(), "ubuntu");
    assert_eq!(image.to_string(), "ubuntu");
}

#[test]
fn oci_image_ref_omits_absent_tag_and_digest_from_json() -> TestResult {
    let bare = OciImageRef::parse_unqualified("ubuntu");
    let wire = serde_json::to_value(&bare)?;
    assert_eq!(wire, json!({ "registry": "", "repository": "ubuntu" }));
    assert_eq!(serde_json::from_value::<OciImageRef>(wire)?, bare);

    let pinned = digest('b')?;
    let full = OciImageRef {
        registry: "ghcr.io".to_owned(),
        repository: "team-a/app".to_owned(),
        tag: Some("1.0".to_owned()),
        digest: Some(pinned.clone()),
    };
    let wire = serde_json::to_value(&full)?;
    assert_eq!(
        wire,
        json!({
            "registry": "ghcr.io",
            "repository": "team-a/app",
            "tag": "1.0",
            "digest": pinned.as_str(),
        })
    );
    assert_eq!(serde_json::from_value::<OciImageRef>(wire)?, full);
    Ok(())
}

#[test]
fn oci_image_ref_rejects_an_invalid_digest_on_deserialization() {
    let wire = json!({ "registry": "", "repository": "app", "digest": "sha256:short" });
    assert!(serde_json::from_value::<OciImageRef>(wire).is_err());
}

#[test]
fn oci_artifact_ref_roundtrips_and_formats_its_allowlist_string() -> TestResult {
    let pinned = digest('c')?;
    let artifact = OciArtifactRef {
        registry: "ghcr.io".to_owned(),
        repository: "snapshots/acme".to_owned(),
        digest: pinned.clone(),
        artifact_type: ArtifactType::SandboxRootfsV1,
    };
    assert_eq!(artifact.allowlist_string(), format!("ghcr.io/snapshots/acme@{pinned}"));
    let wire = serde_json::to_value(&artifact)?;
    assert_eq!(
        wire,
        json!({
            "registry": "ghcr.io",
            "repository": "snapshots/acme",
            "digest": pinned.as_str(),
            "artifact_type": "sandbox_rootfs_v1",
        })
    );
    assert_eq!(serde_json::from_value::<OciArtifactRef>(wire)?, artifact);
    Ok(())
}

#[test]
fn native_snapshot_ref_roundtrips_with_and_without_expiry() -> TestResult {
    let reference = NativeSnapshotRef {
        provider: ProviderKind::Daytona,
        id: NativeSnapshotId::new("trogon-snap-1")?,
        expires_at: Some(1_900_000_000),
    };
    let wire = serde_json::to_value(&reference)?;
    assert_eq!(
        wire,
        json!({ "provider": "daytona", "id": "trogon-snap-1", "expires_at": 1_900_000_000u64 })
    );
    assert_eq!(serde_json::from_value::<NativeSnapshotRef>(wire)?, reference);

    let no_expiry: NativeSnapshotRef = serde_json::from_value(json!({ "provider": "daytona", "id": "s" }))?;
    assert_eq!(no_expiry.expires_at, None);
    assert_eq!(
        serde_json::to_value(&no_expiry)?,
        json!({ "provider": "daytona", "id": "s" })
    );
    Ok(())
}

#[test]
fn native_snapshot_ref_reports_expiry() -> TestResult {
    let mut reference = NativeSnapshotRef {
        provider: ProviderKind::Daytona,
        id: NativeSnapshotId::new("ok-id_1.2")?,
        expires_at: Some(100),
    };
    assert!(reference.is_expired_at(100));
    assert!(reference.is_expired_at(101));
    assert!(!reference.is_expired_at(99));
    reference.expires_at = None;
    assert!(!reference.is_expired_at(u64::MAX));
    Ok(())
}

#[test]
fn native_snapshot_id_accepts_valid_ids() -> TestResult {
    let longest = "x".repeat(NativeSnapshotId::MAX_LEN);
    for value in ["ok-id_1.2", "s", longest.as_str()] {
        let id = NativeSnapshotId::new(value)?;
        assert_eq!(id.as_str(), value);
        assert_eq!(id.to_string(), value);
        assert_eq!(NativeSnapshotId::from_str(value)?, id);
        assert_eq!(NativeSnapshotId::try_from(value.to_owned())?, id);
        assert_eq!(serde_json::to_value(&id)?, json!(value));
        assert_eq!(serde_json::from_value::<NativeSnapshotId>(json!(value))?, id);
        assert_eq!(String::from(id), value);
    }
    Ok(())
}

#[test]
fn native_snapshot_id_rejects_invalid_ids_at_construction_and_deserialization() {
    let too_long = "x".repeat(NativeSnapshotId::MAX_LEN + 1);
    let invalid_length = NativeSnapshotIdError::InvalidLength {
        max: NativeSnapshotId::MAX_LEN,
    };
    for (value, expected) in [
        ("", invalid_length),
        (too_long.as_str(), invalid_length),
        ("has space", NativeSnapshotIdError::InvalidCharacters),
        ("a/b", NativeSnapshotIdError::InvalidCharacters),
        ("caf\u{e9}", NativeSnapshotIdError::InvalidCharacters),
    ] {
        assert_eq!(NativeSnapshotId::new(value), Err(expected), "{value:?}");
        let error = serde_json::from_value::<NativeSnapshotId>(json!(value)).err();
        assert_eq!(
            error.map(|error| error.to_string()),
            Some(expected.to_string()),
            "{value:?}"
        );
    }
    let error = serde_json::from_value::<NativeSnapshotRef>(json!({ "provider": "daytona", "id": "a/b" })).err();
    assert_eq!(
        error.map(|error| error.to_string()),
        Some(NativeSnapshotIdError::InvalidCharacters.to_string())
    );
}

#[test]
fn native_snapshot_id_errors_keep_their_messages() {
    assert_eq!(
        NativeSnapshotIdError::InvalidLength { max: 128 }.to_string(),
        "native snapshot id must be 1 to 128 characters"
    );
    assert_eq!(
        NativeSnapshotIdError::InvalidCharacters.to_string(),
        "native snapshot id may only contain ASCII letters, digits, '-', '_' and '.'"
    );
}

#[test]
fn native_snapshot_receipt_omits_an_unreported_size() -> TestResult {
    let reference = NativeSnapshotRef {
        provider: ProviderKind::Modal,
        id: NativeSnapshotId::new("s")?,
        expires_at: None,
    };
    let unsized_receipt = NativeSnapshotReceipt {
        reference: reference.clone(),
        bytes: None,
    };
    let wire = serde_json::to_value(&unsized_receipt)?;
    assert_eq!(wire, json!({ "reference": { "provider": "modal", "id": "s" } }));
    assert_eq!(serde_json::from_value::<NativeSnapshotReceipt>(wire)?, unsized_receipt);

    let sized = NativeSnapshotReceipt {
        reference,
        bytes: Some(4096),
    };
    let wire = serde_json::to_value(&sized)?;
    assert_eq!(
        wire,
        json!({ "reference": { "provider": "modal", "id": "s" }, "bytes": 4096 })
    );
    assert_eq!(serde_json::from_value::<NativeSnapshotReceipt>(wire)?, sized);
    Ok(())
}

#[test]
fn pull_secret_and_service_account_ids_serialize_as_unchecked_json_strings() -> TestResult {
    for value in ["test-secret", "", "any value: / \n"] {
        let secret = PullSecretId::new(value);
        assert_eq!(secret.as_str(), value);
        assert_eq!(secret.to_string(), value);
        assert_eq!(PullSecretId::from(value.to_owned()), secret);
        assert_eq!(serde_json::to_value(&secret)?, json!(value));
        assert_eq!(serde_json::from_value::<PullSecretId>(json!(value))?, secret);
        assert_eq!(String::from(secret), value);

        let account = ServiceAccountId::new(value);
        assert_eq!(account.as_str(), value);
        assert_eq!(account.to_string(), value);
        assert_eq!(ServiceAccountId::from(value.to_owned()), account);
        assert_eq!(serde_json::to_value(&account)?, json!(value));
        assert_eq!(serde_json::from_value::<ServiceAccountId>(json!(value))?, account);
        assert_eq!(String::from(account), value);
    }
    for value in [json!(1), json!(null), json!({})] {
        assert!(
            serde_json::from_value::<PullSecretId>(value.clone()).is_err(),
            "{value}"
        );
        assert!(
            serde_json::from_value::<ServiceAccountId>(value.clone()).is_err(),
            "{value}"
        );
    }
    Ok(())
}
