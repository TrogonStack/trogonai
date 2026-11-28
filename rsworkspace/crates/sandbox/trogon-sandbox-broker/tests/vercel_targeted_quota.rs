//! Exercises real broker/NATS quota admission and signed assigned Vercel transport.
//! The synthetic worker stub creates no Vercel sandbox. Secure client authorization
//! and the Vercel worker's own ledger are separate contracts.

use std::error::Error;

use trogon_sandbox_contract::{IsolationTier, ProviderKind};

mod targeted_quota_support;

use targeted_quota_support::Scenario;

static VERCEL: Scenario = Scenario {
    provider: ProviderKind::Vercel,
    tier: IsolationTier::MicroVm,
    worker: "quota-assigned-vercel",
    quota_bucket: "vercel-targeted-quota",
    image_bucket: "vercel-targeted-images",
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_vercel_targeted_lease_at_the_finite_cpu_cap_is_reserved_and_refunded_once() -> Result<(), Box<dyn Error>> {
    targeted_quota_support::run(&VERCEL).await
}
