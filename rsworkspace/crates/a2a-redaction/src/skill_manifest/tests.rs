use std::path::PathBuf;

use tempfile::TempDir;

use crate::a2a_method::A2aMethod;
use crate::skill_id::SkillId;
use crate::skill_manifest::{
    JsonPathExpr, SkillCategory, SkillManifestError, SkillManifestRegistry, SkillManifestVersion, SkillMethodMatcher,
    SkillSelectionPlan,
};

fn bundled_skills_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../a2a-pack/skills")
}

fn write_manifest(dir: &TempDir, skill_id: &str, body: &str) -> PathBuf {
    let path = dir.path().join(format!("{skill_id}.skill.toml"));
    std::fs::write(&path, body).unwrap();
    path
}

const VALID_MANIFEST: &str = r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "OneOf", methods = ["message/send", "message/stream"] }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#;

#[test]
fn valid_manifest_parses() {
    let dir = TempDir::new().unwrap();
    write_manifest(&dir, "pii.email_mask.v1", VALID_MANIFEST);
    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let manifest = registry
        .lookup(&SkillId::new("pii.email_mask.v1").expect("valid"))
        .unwrap();
    assert_eq!(manifest.skill_id().as_str(), "pii.email_mask.v1");
    assert_eq!(manifest.version().as_str(), "1.0.0");
}

#[test]
fn missing_skill_id_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "broken.skill",
        r#"
wasm_path = "skills/x.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(
        err,
        SkillManifestError::MissingField { field: "skill_id", .. }
    ));
}

#[test]
fn bad_version_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "not-semver"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::InvalidVersion { .. }));
}

#[test]
fn duplicate_skill_id_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(&dir, "pii.email_mask.v1", VALID_MANIFEST);
    let mut registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let duplicate = registry
        .lookup(&SkillId::new("pii.email_mask.v1").expect("valid"))
        .unwrap()
        .clone();
    let err = registry.insert(duplicate).unwrap_err();
    assert!(matches!(err, SkillManifestError::DuplicateSkillId { .. }));
}

#[test]
fn skills_for_method_filters_one_of() {
    let dir = TempDir::new().unwrap();
    write_manifest(&dir, "pii.email_mask.v1", VALID_MANIFEST);
    write_manifest(
        &dir,
        "tasks.only.v1",
        r#"
skill_id = "tasks.only.v1"
wasm_path = "skills/tasks_only.wasm"
applies_to_method = { kind = "OneOf", methods = ["tasks/get"] }
applies_to_paths = ["$.task.id"]
category = { Custom = "tasks" }
version = "1.0.0"
"#,
    );
    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let send_matches = registry.skills_for_method(&A2aMethod::MessageSend);
    assert_eq!(send_matches.len(), 1);
    assert_eq!(send_matches[0].skill_id().as_str(), "pii.email_mask.v1");

    let task_matches = registry.skills_for_method(&A2aMethod::TasksGet);
    assert_eq!(task_matches.len(), 1);
    assert_eq!(task_matches[0].skill_id().as_str(), "tasks.only.v1");
}

#[test]
fn skill_selection_plan_order_is_deterministic() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    write_manifest(
        &dir,
        "credentials.bearer_redact.v1",
        r#"
skill_id = "credentials.bearer_redact.v1"
wasm_path = "skills/credentials_bearer_redact.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Credentials"
version = "1.0.0"
"#,
    );
    write_manifest(
        &dir,
        "internal_route.x_internal_strip.v1",
        r#"
skill_id = "internal_route.x_internal_strip.v1"
wasm_path = "skills/internal_route_x_internal_strip.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.metadata"]
category = "InternalRoute"
version = "1.0.0"
"#,
    );

    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let payload_paths = [
        JsonPathExpr::new("$.message.parts[*].text"),
        JsonPathExpr::new("$.message.metadata"),
    ];
    let plan_a = SkillSelectionPlan::plan(&registry, &A2aMethod::MessageSend, &payload_paths);
    let plan_b = SkillSelectionPlan::plan(&registry, &A2aMethod::MessageSend, &payload_paths);

    let ids_a: Vec<_> = plan_a
        .manifests()
        .iter()
        .map(|manifest| manifest.skill_id().as_str())
        .collect();
    let ids_b: Vec<_> = plan_b
        .manifests()
        .iter()
        .map(|manifest| manifest.skill_id().as_str())
        .collect();
    assert_eq!(ids_a, ids_b);
    assert_eq!(
        ids_a,
        vec![
            "internal_route.x_internal_strip.v1",
            "credentials.bearer_redact.v1",
            "pii.email_mask.v1",
        ]
    );
}

#[test]
#[ignore = "depends on a2a-pack/skills/*.skill.toml fixtures landing in a later extraction slice"]
fn reference_skills_in_a2a_pack_parse() {
    let dir = bundled_skills_dir();
    let registry = SkillManifestRegistry::load_from_dir(&dir).unwrap();
    assert!(
        registry
            .lookup(&SkillId::new("pii.email_mask.v1").expect("valid"))
            .is_some()
    );
    assert!(
        registry
            .lookup(&SkillId::new("credentials.bearer_redact.v1").expect("valid"))
            .is_some()
    );
    assert!(
        registry
            .lookup(&SkillId::new("internal_route.x_internal_strip.v1").expect("valid"))
            .is_some()
    );
    assert!(
        registry
            .lookup(&SkillId::new("pii-regex-redactor").expect("valid"))
            .is_some()
    );
    assert!(
        registry
            .lookup(&SkillId::new("secrets-redactor").expect("valid"))
            .is_some()
    );
    assert!(
        registry
            .lookup(&SkillId::new("json-path-sanitizer").expect("valid"))
            .is_some()
    );
}

#[test]
fn semver_version_accepts_prerelease() {
    let version = SkillManifestVersion::new("1.2.3-rc.1").unwrap();
    assert_eq!(version.as_str(), "1.2.3-rc.1");
}

#[test]
fn filename_mismatch_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(&dir, "wrong-name.skill", VALID_MANIFEST);
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::FilenameMismatch { .. }));
}

#[test]
fn wasm_path_rejects_parent_dir_escape() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "../outside.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::WasmPathEscapesBundle { .. }));
}

#[test]
fn wasm_path_rejects_absolute_path() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "/etc/passwd.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::WasmPathEscapesBundle { .. }));
}

#[test]
fn empty_applies_to_paths_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = []
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::EmptyPaths { .. }));
}

#[test]
fn one_of_without_methods_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "OneOf", methods = [] }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::EmptyMethods { .. }));
}

#[test]
fn invalid_category_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "NotARealCategory"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::InvalidCategory { .. }));
}

#[test]
fn any_matcher_with_methods_list_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "Any", methods = ["message/send"] }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::InvalidMethod { .. }));
}

#[test]
fn resolve_wasm_path_joins_bundle_dir() {
    let dir = TempDir::new().unwrap();
    write_manifest(&dir, "pii.email_mask.v1", VALID_MANIFEST);
    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let manifest = registry
        .lookup(&SkillId::new("pii.email_mask.v1").expect("valid"))
        .unwrap();
    let resolved = manifest.resolve_wasm_path(dir.path());
    assert!(resolved.ends_with("skills/pii_email_mask.wasm"));
}

#[test]
fn selection_plan_is_empty_when_paths_do_not_overlap() {
    let dir = TempDir::new().unwrap();
    write_manifest(&dir, "pii.email_mask.v1", VALID_MANIFEST);
    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let plan = SkillSelectionPlan::plan(
        &registry,
        &A2aMethod::MessageSend,
        &[JsonPathExpr::new("$.other.field")],
    );
    assert!(plan.is_empty());
}

#[test]
fn invalid_method_in_one_of_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "OneOf", methods = ["not/a/method"] }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::InvalidMethod { .. }));
}

#[test]
fn custom_category_named_prefix_parses() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "custom.skill.v1",
        r#"
skill_id = "custom.skill.v1"
wasm_path = "skills/custom.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Custom:my-tag"
version = "1.0.0"
"#,
    );
    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let manifest = registry
        .lookup(&SkillId::new("custom.skill.v1").expect("valid"))
        .unwrap();
    assert!(matches!(
        manifest.category(),
        SkillCategory::Custom(tag) if tag == "my-tag"
    ));
}

#[test]
fn missing_wasm_path_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(
        err,
        SkillManifestError::MissingField { field: "wasm_path", .. }
    ));
}

#[test]
fn missing_applies_to_method_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(
        err,
        SkillManifestError::MissingField {
            field: "applies_to_method",
            ..
        }
    ));
}

#[test]
fn missing_applies_to_paths_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "Any" }
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(
        err,
        SkillManifestError::MissingField {
            field: "applies_to_paths",
            ..
        }
    ));
}

#[test]
fn missing_category_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(
        err,
        SkillManifestError::MissingField { field: "category", .. }
    ));
}

#[test]
fn missing_version_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::MissingField { field: "version", .. }));
}

#[test]
fn invalid_skill_id_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "escape.v1",
        r#"
skill_id = "a/b"
wasm_path = "skills/x.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::InvalidSkillId { .. }));
}

#[test]
fn malformed_toml_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = [
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::Parse { .. }));
}

#[test]
fn load_from_missing_dir_errors() {
    let err = SkillManifestRegistry::load_from_dir(PathBuf::from("/no/such/skill/manifest/dir").as_path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::Io { .. }));
}

#[test]
fn duplicate_insert_without_source_uses_wasm_path() {
    let dir = TempDir::new().unwrap();
    write_manifest(&dir, "pii.email_mask.v1", VALID_MANIFEST);
    let loaded = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let manifest = loaded
        .lookup(&SkillId::new("pii.email_mask.v1").expect("valid"))
        .unwrap()
        .clone();

    let mut registry = SkillManifestRegistry::new();
    registry.insert(manifest.clone()).unwrap();
    let err = registry.insert(manifest).unwrap_err();
    assert!(matches!(err, SkillManifestError::DuplicateSkillId { .. }));
}

#[test]
fn json_path_expr_display_matches_as_str() {
    let expr = JsonPathExpr::new("$.message.parts[*].text");
    assert_eq!(expr.as_str(), "$.message.parts[*].text");
    assert_eq!(expr.to_string(), "$.message.parts[*].text");
}

#[test]
fn registry_default_insert_and_manifests_iterator() {
    let dir = TempDir::new().unwrap();
    write_manifest(&dir, "pii.email_mask.v1", VALID_MANIFEST);
    let loaded = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let manifest = loaded
        .lookup(&SkillId::new("pii.email_mask.v1").expect("valid"))
        .unwrap()
        .clone();

    let mut registry = SkillManifestRegistry::default();
    registry.insert(manifest).unwrap();
    assert_eq!(registry.manifests().count(), 1);
    assert!(
        registry
            .lookup(&SkillId::new("pii.email_mask.v1").expect("valid"))
            .is_some()
    );
}

#[test]
fn manifest_accessors_expose_wasm_and_paths() {
    let dir = TempDir::new().unwrap();
    write_manifest(&dir, "pii.email_mask.v1", VALID_MANIFEST);
    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let manifest = registry
        .lookup(&SkillId::new("pii.email_mask.v1").expect("valid"))
        .unwrap();

    assert_eq!(
        manifest.wasm_path().as_path().to_str().unwrap(),
        "skills/pii_email_mask.wasm"
    );
    assert_eq!(manifest.applies_to_paths().len(), 1);
    assert_eq!(manifest.applies_to_paths()[0].as_str(), "$.message.parts[*].text");
}

#[test]
fn string_any_matcher_parses() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = "Any"
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let manifest = registry
        .lookup(&SkillId::new("pii.email_mask.v1").expect("valid"))
        .unwrap();
    assert!(matches!(manifest.applies_to_method(), SkillMethodMatcher::Any));
    assert!(manifest.applies_to_method().matches(&A2aMethod::TasksGet));
}

#[test]
fn invalid_any_string_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = "Sometimes"
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::InvalidMethod { .. }));
}

#[test]
fn invalid_method_kind_errors() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "Sometimes", methods = ["message/send"] }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    let err = SkillManifestRegistry::load_from_dir(dir.path()).unwrap_err();
    assert!(matches!(err, SkillManifestError::InvalidMethod { .. }));
}

#[test]
fn rate_limit_category_parses() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "rate.limit.v1",
        r#"
skill_id = "rate.limit.v1"
wasm_path = "skills/rate_limit.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "RateLimit"
version = "1.0.0"
"#,
    );
    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let manifest = registry.lookup(&SkillId::new("rate.limit.v1").expect("valid")).unwrap();
    assert!(matches!(manifest.category(), SkillCategory::RateLimit));
}

#[test]
fn load_from_dir_skips_non_skill_toml_files() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "ignore me").unwrap();
    std::fs::write(dir.path().join("other.toml"), "key = \"value\"").unwrap();
    write_manifest(&dir, "pii.email_mask.v1", VALID_MANIFEST);

    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    assert_eq!(registry.manifests().count(), 1);
}

#[test]
fn lookup_returns_none_for_missing_skill() {
    let registry = SkillManifestRegistry::new();
    assert!(
        registry
            .lookup(&SkillId::new("missing.skill.v1").expect("valid"))
            .is_none()
    );
}

#[test]
fn semver_version_accepts_build_metadata() {
    let version = SkillManifestVersion::new("1.2.3+build.42").unwrap();
    assert_eq!(version.as_str(), "1.2.3+build.42");
}

#[test]
fn semver_version_rejects_incomplete_shape() {
    let err = SkillManifestVersion::new("1.2").unwrap_err();
    assert!(matches!(err, SkillManifestError::InvalidVersion { .. }));
}

#[test]
fn selection_plan_filters_by_method_and_paths() {
    let dir = TempDir::new().unwrap();
    write_manifest(&dir, "pii.email_mask.v1", VALID_MANIFEST);
    write_manifest(
        &dir,
        "rate.limit.v1",
        r#"
skill_id = "rate.limit.v1"
wasm_path = "skills/rate_limit.wasm"
applies_to_method = { kind = "OneOf", methods = ["tasks/get"] }
applies_to_paths = ["$.message.parts[*].text"]
category = "RateLimit"
version = "1.0.0"
"#,
    );
    write_manifest(
        &dir,
        "custom.tag.v1",
        r#"
skill_id = "custom.tag.v1"
wasm_path = "skills/custom.wasm"
applies_to_method = { kind = "OneOf", methods = ["message/send"] }
applies_to_paths = ["$.message.parts[*].text"]
category = { Custom = "tag" }
version = "1.0.0"
"#,
    );

    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let payload_paths = [JsonPathExpr::new("$.message.parts[*].text")];

    let send_plan = SkillSelectionPlan::plan(&registry, &A2aMethod::MessageSend, &payload_paths);
    let send_ids: Vec<_> = send_plan
        .manifests()
        .iter()
        .map(|manifest| manifest.skill_id().as_str())
        .collect();
    assert_eq!(send_ids, vec!["pii.email_mask.v1", "custom.tag.v1"]);

    let task_plan = SkillSelectionPlan::plan(&registry, &A2aMethod::TasksGet, &payload_paths);
    let task_ids: Vec<_> = task_plan
        .manifests()
        .iter()
        .map(|manifest| manifest.skill_id().as_str())
        .collect();
    assert_eq!(task_ids, vec!["rate.limit.v1"]);

    let no_overlap = SkillSelectionPlan::plan(&registry, &A2aMethod::MessageSend, &[JsonPathExpr::new("$.missing")]);
    assert!(no_overlap.is_empty());
}

/// Two registries loaded from separate directories with the same skill_id
/// cannot be merged via insert — the second insert must return DuplicateSkillId.
#[test]
fn load_from_dir_rejects_duplicate_skill_id_across_files() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();
    write_manifest(&dir1, "pii.email_mask.v1", VALID_MANIFEST);
    write_manifest(&dir2, "pii.email_mask.v1", VALID_MANIFEST);

    let reg1 = SkillManifestRegistry::load_from_dir(dir1.path()).unwrap();
    let mut reg2 = SkillManifestRegistry::load_from_dir(dir2.path()).unwrap();
    let manifest = reg1
        .lookup(&SkillId::new("pii.email_mask.v1").expect("valid"))
        .unwrap()
        .clone();
    let err = reg2.insert(manifest).unwrap_err();
    assert!(matches!(err, SkillManifestError::DuplicateSkillId { .. }));
}

/// RateLimit skills should sort after InternalRoute, Credentials and Pii but
/// before Custom — exercising the priority() method for RateLimit.
#[test]
fn skills_for_method_orders_rate_limit_after_pii_before_custom() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "rate.limit.v1",
        r#"
skill_id = "rate.limit.v1"
wasm_path = "skills/rate_limit.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "RateLimit"
version = "1.0.0"
"#,
    );
    write_manifest(
        &dir,
        "pii.email_mask.v1",
        r#"
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    write_manifest(
        &dir,
        "custom.hook.v1",
        r#"
skill_id = "custom.hook.v1"
wasm_path = "skills/custom.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = { Custom = "hook" }
version = "1.0.0"
"#,
    );

    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let matches = registry.skills_for_method(&A2aMethod::MessageSend);
    let ids: Vec<_> = matches.iter().map(|m| m.skill_id().as_str()).collect();
    // Pii (2) < RateLimit (3) < Custom (4)
    assert_eq!(ids, vec!["pii.email_mask.v1", "rate.limit.v1", "custom.hook.v1"]);
}

#[test]
fn skills_for_method_sorts_same_category_by_skill_id() {
    let dir = TempDir::new().unwrap();
    write_manifest(
        &dir,
        "pii.z_last.v1",
        r#"
skill_id = "pii.z_last.v1"
wasm_path = "skills/pii_z.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );
    write_manifest(
        &dir,
        "pii.a_first.v1",
        r#"
skill_id = "pii.a_first.v1"
wasm_path = "skills/pii_a.wasm"
applies_to_method = { kind = "Any" }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
"#,
    );

    let registry = SkillManifestRegistry::load_from_dir(dir.path()).unwrap();
    let matches = registry.skills_for_method(&A2aMethod::MessageSend);
    let ids: Vec<_> = matches.iter().map(|manifest| manifest.skill_id().as_str()).collect();
    assert_eq!(ids, vec!["pii.a_first.v1", "pii.z_last.v1"]);
}
