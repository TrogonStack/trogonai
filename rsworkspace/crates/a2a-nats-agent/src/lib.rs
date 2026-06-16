pub mod runtime;

pub use runtime::RuntimeError;

use a2a_nats::server::A2aHandler;
use trogon_std::env::SystemEnv;

pub async fn run<H: A2aHandler>(handler: H) -> Result<(), RuntimeError> {
    runtime::run_with_env(handler, &SystemEnv).await
}
