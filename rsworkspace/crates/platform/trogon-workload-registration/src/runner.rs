mod cli_request;
mod command_wire;
mod configuration_wire;
mod controller_pipe;
mod operation;
mod runner_config;
mod runner_error;
mod shutdown;
mod signed_frame_wire;
mod timeout_cause;
mod wait_limit;

use clap::Parser;
use clap::error::ErrorKind;
use trogon_std::SystemEnv;
use trogon_workload_registration::{
    ProducerStatus, ProductionRegistrationProducer, production_emergency_withdrawal, provision_streams,
};

use cli_request::CliRequest;
use controller_pipe::ControllerPipe;
use operation::Operation;
use runner_config::RunnerConfig;
use runner_error::RunnerError;
use shutdown::Shutdown;
use wait_limit::WaitLimit;

pub async fn run() -> Result<(), RunnerError> {
    let request = match CliRequest::try_parse() {
        Ok(request) => request,
        Err(error) if matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            error.print().map_err(RunnerError::Output)?;
            return Ok(());
        }
        Err(error) => return Err(RunnerError::Arguments(error)),
    };
    let (operation, wire) = request.into_parts();
    let config = RunnerConfig::from_wire(wire, &SystemEnv)?;
    let mut shutdown = Shutdown::new()?;
    tokio::select! {
        biased;
        () = shutdown.wait() => Err(RunnerError::Cancelled),
        result = dispatch(operation, config) => result,
    }
}

async fn dispatch(operation: Operation, config: RunnerConfig) -> Result<(), RunnerError> {
    match operation {
        Operation::Provision => {
            WaitLimit::effect(None, None)
                .run(async {
                    provision_streams(config.nats(), config.producer())
                        .await
                        .map_err(RunnerError::producer)
                })
                .await
        }
        Operation::Serve => serve(config).await,
        Operation::Withdraw => withdraw(config).await,
    }
}

async fn serve(config: RunnerConfig) -> Result<(), RunnerError> {
    let mut controller = ControllerPipe::from_stdin()?;
    let initial = controller.next(None).await?.verify(config.producer())?;
    let deadline = initial.lease().deadline();
    let mut producer = WaitLimit::effect(Some(deadline), None)
        .run(async {
            ProductionRegistrationProducer::start(config.producer().clone(), config.nats().clone(), initial)
                .await
                .map_err(RunnerError::producer)
        })
        .await?;
    loop {
        if producer.status() != ProducerStatus::Active {
            return Err(RunnerError::Poisoned);
        }
        let current_deadline = producer.current_deadline();
        let next = controller
            .next(Some(current_deadline))
            .await?
            .verify(config.producer())?;
        let deadline = next.lease().deadline();
        WaitLimit::effect(Some(deadline), Some(current_deadline))
            .run(async { producer.apply(next).await.map_err(RunnerError::producer) })
            .await?;
    }
}

async fn withdraw(config: RunnerConfig) -> Result<(), RunnerError> {
    let mut controller = ControllerPipe::from_stdin()?;
    let denial = controller.next(None).await?.verify(config.producer())?;
    if !denial.bindings().is_empty() && !denial.roots().is_empty() {
        return Err(RunnerError::ExpectedDenial);
    }
    let deadline = denial.lease().deadline();
    WaitLimit::effect(Some(deadline), None)
        .run(async {
            production_emergency_withdrawal(config.nats(), config.producer(), denial)
                .await
                .map_err(RunnerError::producer)
        })
        .await?;
    Ok(())
}
