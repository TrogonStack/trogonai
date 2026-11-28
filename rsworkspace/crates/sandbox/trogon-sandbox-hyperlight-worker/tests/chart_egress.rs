//! The worker's Helm chart claims at least the egress enforcement the capability profile advertises.

use std::error::Error;

use trogon_sandbox_conformance::ensure_chart_egress_at_least_profile;
use trogon_sandbox_hyperlight_worker::capability_profile;

const CHART: &str = "trogon-sandbox-hyperlight-worker";

#[test]
fn the_helm_chart_egress_label_covers_the_profile() -> Result<(), Box<dyn Error>> {
    let profile = capability_profile(1, 1);
    ensure_chart_egress_at_least_profile(env!("CARGO_MANIFEST_DIR"), CHART, profile.isolation.egress_enforcement)?;
    Ok(())
}
