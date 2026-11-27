use clap::Parser;
use trogon_sandbox_contract::IsolationTier;

use super::*;
use crate::test_fixtures::TestResult;

#[test]
fn tier_runtime_classes_override_the_default() -> TestResult {
    let args = WorkerArgs::try_parse_from([
        "worker",
        "--k8s-namespace",
        "sandboxes",
        "--image",
        "registry.example/base",
        "--runtime-class",
        "default-class",
        "--runtime-class-micro-vm",
        "micro-class",
        "--runtime-class-user-space-kernel",
        "kernel-class",
    ])?;
    let config = args.k8s_config(&"worker-0".parse()?);
    assert_eq!(config.runtime_class_for(Some(IsolationTier::MicroVm)), "micro-class");
    assert_eq!(
        config.runtime_class_for(Some(IsolationTier::UserSpaceKernel)),
        "kernel-class"
    );
    assert_eq!(config.runtime_class_for(None), "default-class");
    Ok(())
}
