//! Isolates real broker/NATS quota admission and signed assigned transport.
//! The synthetic worker grants no native resource. Secure client authorization
//! is exercised separately by the composed SDK acceptance target.

use std::error::Error;

use trogon_sandbox_contract::{IsolationTier, ProviderKind};

mod targeted_quota_support;

use targeted_quota_support::Scenario;

static E2B: Scenario = Scenario {
    provider: ProviderKind::E2b,
    tier: IsolationTier::MicroVm,
    worker: "quota-assigned-e2b",
    quota_bucket: "e2b-targeted-quota",
    image_bucket: "e2b-targeted-images",
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_e2b_assigned_lease_at_the_finite_cpu_cap_is_reserved_and_refunded_once() -> Result<(), Box<dyn Error>> {
    targeted_quota_support::run(&E2B).await
}
