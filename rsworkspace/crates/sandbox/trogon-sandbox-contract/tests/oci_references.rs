use serde_json::json;
use trogon_sandbox_contract::{
    ArtifactType, Digest, NativeSnapshotIdError, NativeSnapshotReceipt, NativeSnapshotRef, OciArtifactRef, OciImageRef,
    ProviderKind, PullSecretId, ServiceAccountId,
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
        id: "trogon-snap-1".to_owned(),
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
fn native_snapshot_ref_validates_id_and_expiry() {
    let mut reference = NativeSnapshotRef {
        provider: ProviderKind::Daytona,
        id: "ok-id_1.2".to_owned(),
        expires_at: Some(100),
    };
    assert_eq!(reference.validate(), Ok(()));
    assert!(reference.is_expired_at(100));
    assert!(reference.is_expired_at(101));
    assert!(!reference.is_expired_at(99));
    reference.expires_at = None;
    assert!(!reference.is_expired_at(u64::MAX));

    reference.id = "x".repeat(NativeSnapshotRef::MAX_ID_LEN);
    assert_eq!(reference.validate(), Ok(()));

    let too_long = "x".repeat(NativeSnapshotRef::MAX_ID_LEN + 1);
    for (id, expected) in [
        (
            "",
            NativeSnapshotIdError::InvalidLength {
                max: NativeSnapshotRef::MAX_ID_LEN,
            },
        ),
        (
            too_long.as_str(),
            NativeSnapshotIdError::InvalidLength {
                max: NativeSnapshotRef::MAX_ID_LEN,
            },
        ),
        ("has space", NativeSnapshotIdError::InvalidCharacters),
        ("a/b", NativeSnapshotIdError::InvalidCharacters),
        ("caf\u{e9}", NativeSnapshotIdError::InvalidCharacters),
    ] {
        reference.id = id.to_owned();
        assert_eq!(reference.validate(), Err(expected), "{id:?}");
    }
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
        id: "s".to_owned(),
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
