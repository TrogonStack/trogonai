//! Exercises real broker/NATS quota admission and signed assigned Daytona transport.
//! The synthetic worker stub creates no Daytona sandbox. Secure client authorization
//! and the Daytona worker's own ledger are separate contracts.

use std::error::Error;

use trogon_sandbox_contract::{IsolationTier, ProviderKind};

mod targeted_quota_support;

use targeted_quota_support::Scenario;

static DAYTONA: Scenario = Scenario {
    provider: ProviderKind::Daytona,
    tier: IsolationTier::Container,
    worker: "quota-assigned-daytona",
    quota_bucket: "daytona-targeted-quota",
    image_bucket: "daytona-targeted-images",
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_daytona_targeted_lease_at_the_finite_cpu_cap_is_reserved_and_refunded_once() -> Result<(), Box<dyn Error>> {
    targeted_quota_support::run(&DAYTONA).await
}
