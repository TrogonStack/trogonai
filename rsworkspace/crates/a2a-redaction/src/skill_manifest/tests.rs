use std::path::PathBuf;

use tempfile::TempDir;

use crate::a2a_method::A2aMethod;
use crate::skill_id::SkillId;
use crate::skill_manifest::{
    JsonPathExpr, SkillManifestError, SkillManifestRegistry, SkillManifestVersion, SkillSelectionPlan,
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
    write_manifest(
        &dir,
        "wrong-name.skill",
        VALID_MANIFEST,
    );
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
        crate::skill_manifest::SkillCategory::Custom(tag) if tag == "my-tag"
    ));
}
