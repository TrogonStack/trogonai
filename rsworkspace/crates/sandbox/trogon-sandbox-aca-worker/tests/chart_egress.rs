//! The worker's Helm chart claims the egress enforcement the capability profile advertises.

use std::error::Error;

use trogon_sandbox_aca_worker::PROFILE;
use trogon_sandbox_conformance::ensure_chart_egress_matches_profile;

const CHART: &str = "trogon-sandbox-aca-worker";

#[test]
fn the_helm_chart_egress_label_matches_the_profile() -> Result<(), Box<dyn Error>> {
    ensure_chart_egress_matches_profile(env!("CARGO_MANIFEST_DIR"), CHART, PROFILE.isolation.egress_enforcement)?;
    Ok(())
}
