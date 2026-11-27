//! Exercises real broker/NATS quota admission and signed assigned Docker transport.
//! The synthetic worker stub creates no Docker resource. Secure client authorization
//! and native Docker acceptance remain separate contracts.

use std::error::Error;

use trogon_sandbox_contract::{IsolationTier, ProviderKind};

mod targeted_quota_support;

use targeted_quota_support::Scenario;

static DOCKER: Scenario = Scenario {
    provider: ProviderKind::LocalDocker,
    tier: IsolationTier::Container,
    worker: "quota-assigned-docker",
    quota_bucket: "docker-targeted-quota",
    image_bucket: "docker-targeted-images",
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_docker_targeted_lease_at_the_finite_cpu_cap_is_reserved_and_refunded_once() -> Result<(), Box<dyn Error>> {
    targeted_quota_support::run(&DOCKER).await
}
