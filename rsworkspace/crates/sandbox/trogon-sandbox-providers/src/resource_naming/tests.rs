use std::collections::BTreeMap;

use crate::{K8sResourceNaming, K8sResourceNamingError};

#[test]
fn sandbox_naming_is_the_default() {
    assert_eq!(K8sResourceNaming::default(), K8sResourceNaming::Sandbox);
}

#[test]
fn sandbox_naming_uses_the_sandbox_family() {
    let naming = K8sResourceNaming::Sandbox;

    assert_eq!(naming.managed_by(), "sandbox-worker");
    assert_eq!(naming.job_label(), "sandbox.trogon.live/job");
    assert_eq!(naming.repo_label(), "sandbox.trogon.live/repo");
    assert_eq!(naming.egress_label(), "sandbox.trogon.live/egress");
    assert_eq!(naming.agent_name(), "sandbox-agent");
    assert_eq!(naming.warm_pool_name(), "sandbox-warm-pool");
    assert_eq!(naming.secrets_bucket(), "sandbox-secrets");
}

#[test]
fn legacy_naming_keeps_the_names_of_resources_created_before_the_rename() {
    let naming = K8sResourceNaming::Legacy;

    assert_eq!(naming.managed_by(), "legacy-runner");
    assert_eq!(naming.job_label(), "legacy/job");
    assert_eq!(naming.repo_label(), "legacy/repo");
    assert_eq!(naming.egress_label(), "legacy/egress");
    assert_eq!(naming.agent_name(), "legacy-agent");
    assert_eq!(naming.warm_pool_name(), "legacy-warm-pool");
    assert_eq!(naming.secrets_bucket(), "legacy-secrets");
}

#[test]
fn each_naming_recognizes_only_its_own_warm_pod_labels() {
    let labels = |manager: &str| BTreeMap::from([("app.kubernetes.io/managed-by".to_string(), manager.to_string())]);

    assert!(K8sResourceNaming::Sandbox.owns_warm_labels(&labels("sandbox-worker")));
    assert!(!K8sResourceNaming::Sandbox.owns_warm_labels(&labels("legacy-runner")));
    assert!(K8sResourceNaming::Legacy.owns_warm_labels(&labels("legacy-runner")));
    assert!(!K8sResourceNaming::Legacy.owns_warm_labels(&BTreeMap::new()));
}

#[test]
fn the_other_naming_is_the_opposite_family() {
    assert_eq!(K8sResourceNaming::Sandbox.other(), K8sResourceNaming::Legacy);
    assert_eq!(K8sResourceNaming::Legacy.other(), K8sResourceNaming::Sandbox);
}

#[test]
fn naming_round_trips_through_its_text_form() -> Result<(), K8sResourceNamingError> {
    for naming in [K8sResourceNaming::Sandbox, K8sResourceNaming::Legacy] {
        assert_eq!(naming.to_string().parse::<K8sResourceNaming>()?, naming);
    }
    assert_eq!(K8sResourceNaming::Sandbox.to_string(), "sandbox");
    assert_eq!(K8sResourceNaming::Legacy.to_string(), "legacy");
    Ok(())
}

#[test]
fn an_unknown_naming_is_rejected_with_a_typed_error() {
    let result = "unknown".parse::<K8sResourceNaming>();

    assert!(matches!(result, Err(K8sResourceNamingError::Unknown(ref value)) if value == "unknown"));
}
