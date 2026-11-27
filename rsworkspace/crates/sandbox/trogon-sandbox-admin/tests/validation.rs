use trogon_sandbox_admin::api_key::{is_raw_token, key_hash_of};
use trogon_sandbox_admin::quota::{parse_month, quota_kv_keys};
use trogon_sandbox_admin::tenant_policy::{validate_host_mount_prefix, validate_pattern};
use trogon_sandbox_admin::{AdminError, parse_artifact_type};
use trogon_sandbox_contract::{ArtifactType, TenantId};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn raw_token_is_64_hex_characters() {
    assert!(is_raw_token(&"a".repeat(64)));
    assert!(is_raw_token(&"0F".repeat(32)));
    assert!(!is_raw_token("abc123"));
    assert!(!is_raw_token(&"g".repeat(64)));
    assert!(!is_raw_token("not-a-hash"));
}

#[test]
fn a_stored_hash_passes_through_unchanged() -> TestResult {
    assert_eq!(key_hash_of("not-a-hash")?, "not-a-hash");
    Ok(())
}

#[test]
fn a_raw_token_is_hashed_before_lookup() -> TestResult {
    let raw = "ab".repeat(32);
    let hashed = key_hash_of(&raw)?;
    assert_ne!(hashed, raw);
    assert_eq!(hashed, key_hash_of(&raw)?);
    Ok(())
}

#[test]
fn image_patterns_allow_one_trailing_wildcard() {
    assert!(validate_pattern("exact:tag").is_ok());
    assert!(validate_pattern("a/b/*").is_ok());
    assert!(matches!(
        validate_pattern("a/*/c"),
        Err(AdminError::MisplacedWildcard { .. })
    ));
    assert!(matches!(validate_pattern(""), Err(AdminError::EmptyPattern)));
}

#[test]
fn host_mount_prefixes_are_absolute() {
    assert!(validate_host_mount_prefix("/srv/workspaces").is_ok());
    assert!(matches!(
        validate_host_mount_prefix(""),
        Err(AdminError::EmptyHostMountPrefix)
    ));
    assert!(matches!(
        validate_host_mount_prefix("srv/workspaces"),
        Err(AdminError::RelativeHostMountPrefix { .. })
    ));
}

#[test]
fn quota_keys_keep_colon_and_underscore_tenants_apart() -> TestResult {
    let colon: TenantId = "org:acme".parse()?;
    let underscore: TenantId = "org_acme".parse()?;
    let (colon_key, colon_legacy) = quota_kv_keys(&colon, 2024, 12);
    let (underscore_key, underscore_legacy) = quota_kv_keys(&underscore, 2024, 12);
    assert_ne!(colon_key, underscore_key);
    assert_eq!(colon_legacy, underscore_legacy);
    assert!(colon_key.ends_with("_2024-12"));
    assert!(colon_legacy.ends_with("_2024-12"));
    Ok(())
}

#[test]
fn month_parses_as_year_and_month() -> TestResult {
    assert_eq!(parse_month("2024-03")?, (2024, 3));
    assert!(matches!(parse_month("2024-13"), Err(AdminError::InvalidMonth(_))));
    assert!(parse_month("March").is_err());
    Ok(())
}

#[test]
fn artifact_types_parse_from_wire_and_type_names() -> TestResult {
    assert_eq!(
        parse_artifact_type("hyperlight_snapshot_v1")?,
        ArtifactType::HyperlightSnapshotV1
    );
    assert_eq!(
        parse_artifact_type("HyperlightSnapshotV1")?,
        ArtifactType::HyperlightSnapshotV1
    );
    assert_eq!(parse_artifact_type("sbom_v1")?, ArtifactType::SbomV1);
    assert!(matches!(
        parse_artifact_type("nonsense"),
        Err(AdminError::UnknownArtifactType { .. })
    ));
    Ok(())
}
