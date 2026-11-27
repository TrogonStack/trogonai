use trogon_sandbox_contract::{
    EgressEnforcementLevel, IsolationTier, ProviderKind, ResourceLimitMechanism, SdkFeature,
};
use trogon_sandbox_v8_wasm_worker::PROFILE;

#[test]
fn profile_advertises_the_v8_wasm_provider_kind() {
    assert_eq!(PROFILE.provider, ProviderKind::V8Wasm);
}

#[test]
fn profile_denies_snapshot_capabilities() {
    assert!(!PROFILE.snapshot.can_snapshot.is_supported());
    assert!(!PROFILE.snapshot.can_checkpoint_running.is_supported());
    assert!(!PROFILE.snapshot.can_restore.is_supported());
    assert!(PROFILE.snapshot.supported_artifact_types.is_empty());
}

#[test]
fn profile_reports_best_effort_egress_and_a_single_isolation_tier() {
    assert_eq!(PROFILE.isolation.egress_enforcement, EgressEnforcementLevel::BestEffort);
    assert_eq!(PROFILE.isolation.default_tier, IsolationTier::WasmMicroVm);
    assert_eq!(PROFILE.isolation.supported_tiers, vec![IsolationTier::WasmMicroVm]);
}

#[test]
fn profile_reports_in_process_runtime_limits() {
    assert_eq!(PROFILE.isolation.resource_limits, ResourceLimitMechanism::Runtime);
}

#[test]
fn profile_does_not_advertise_warm_pool_or_named_tags() {
    assert!(!PROFILE.lifecycle.supports_warm_pool.is_supported());
    assert!(!PROFILE.lifecycle.supports_named_tags.is_supported());
    assert!(!PROFILE.lifecycle.supports_prewarm_per_tenant.is_supported());
}

#[test]
fn profile_advertises_only_evidence_streaming_and_persistent_runtime_features() {
    let features = PROFILE.sdk_features();
    assert!(features.contains(&SdkFeature::EvidenceStreaming));
    assert!(features.contains(&SdkFeature::PersistentRuntime));
    assert!(!features.contains(&SdkFeature::Snapshots));
}
