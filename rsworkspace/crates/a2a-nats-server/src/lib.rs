pub mod runtime;

pub use runtime::RuntimeError;

use a2a_nats::server::A2aExecutor;
use trogon_std::env::SystemEnv;

pub async fn run<H: A2aExecutor>(handler: H) -> Result<(), RuntimeError> {
    runtime::run_with_env(handler, &SystemEnv).await
}
