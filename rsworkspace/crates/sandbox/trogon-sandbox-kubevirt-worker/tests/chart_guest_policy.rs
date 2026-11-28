//! The guest network policy of the stack chart selects the label the worker puts on every VM template.

use std::error::Error;
use std::path::Path;

use trogon_sandbox_kubevirt_worker::GUEST_POLICY_LABEL;

const POLICIES: &str = "../../../../devops/helm/charts/trogon-sandbox/templates/networkpolicies.yaml";
const GUEST_POLICY_NAME: &str = "kubevirt-guests-restrict";

#[test]
fn the_chart_guest_policy_selects_the_vm_template_label() -> Result<(), Box<dyn Error>> {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(POLICIES))?;
    let policy = text
        .split("\n---\n")
        .find(|document| document.contains(GUEST_POLICY_NAME))
        .ok_or("the chart has no guest NetworkPolicy for KubeVirt")?;
    let selector = format!("{GUEST_POLICY_LABEL}: \"true\"");
    let pod_selector = policy
        .split("podSelector:")
        .nth(1)
        .ok_or("the guest policy selects no pods")?;
    assert!(
        pod_selector.contains(&selector),
        "the guest policy does not select {selector}:\n{policy}"
    );
    Ok(())
}
