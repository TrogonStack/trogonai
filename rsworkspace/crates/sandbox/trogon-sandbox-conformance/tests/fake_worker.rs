//! Runs the suite against the in-process worker, one test per execution model.

mod support;

use std::error::Error;

use support::{Config, Exec};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{ExecModel, Target, assert_conforms, checks_for_target, run_suite};
use trogon_sandbox_contract::ProviderKind;

const KIND: ProviderKind = ProviderKind::Subprocess;

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_worker_conforms() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let mut config = Config::new(KIND, Exec::Shell);
    config.lag = 1;
    let _worker = support::spawn(client.clone(), config).await?;
    let target = support::shell_target(client, KIND)?;
    assert_conforms(&target).await?;
    assert!(checks_for_target(&target).len() > 20);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_js_worker_conforms() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let _worker = support::spawn(client.clone(), Config::new(KIND, Exec::JsEval)).await?;
    let mut target = Target::new(client, KIND);
    target.exec = ExecModel::JsEval;
    assert_conforms(&target).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_without_commands_conforms_to_the_lease_checks() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let _worker = support::spawn(client.clone(), Config::new(KIND, Exec::None)).await?;
    let mut target = Target::new(client, KIND);
    target.exec = ExecModel::None;
    target.checkpoint_running_unsupported = true;
    assert_conforms(&target).await?;
    let failures = run_suite(&target).await;
    assert!(failures.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_with_only_a_python_probe_conforms() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let mut config = Config::new(KIND, Exec::Shell);
    config.python_probe = true;
    let _worker = support::spawn(client.clone(), config).await?;
    let target = support::shell_target(client, KIND)?;
    assert_conforms(&target).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_without_a_port_probe_conforms() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let _worker = support::spawn(client.clone(), Config::new(KIND, Exec::Shell)).await?;
    let mut target = support::shell_target(client, KIND)?;
    target.process_listener = None;
    assert_conforms(&target).await?;
    Ok(())
}
