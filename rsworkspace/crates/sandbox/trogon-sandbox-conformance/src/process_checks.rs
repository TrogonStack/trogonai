use trogon_sandbox_contract::{
    ExecutionId, KillCommandResponse, ListProcessesResponse, ProcessState, WaitReadyResponse,
};

use crate::check_failure::ensure;
use crate::held::Held;
use crate::run_fields::{ProcessSpec, Readiness};
use crate::{CheckFailureError, CheckFuture, outcomes};

pub(crate) fn named_process_is_idempotent_listed_and_freed_on_exit<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let argv = &l.target.process_command;
        let spec = || ProcessSpec::named("conformance-svc");
        let first = outcomes::started(l.start_process(argv, spec()).await?)?;
        let again = outcomes::started(l.start_process(argv, spec()).await?)?;
        ensure(first == again, "a second start to return the first", &again)?;
        let listed = l.list_processes().await?;
        let one = only_running_process(&listed, "conformance-svc", &first);
        ensure(one, "one running process", &listed)?;
        let killed = l.kill(&first).await?;
        ensure(matches!(killed, KillCommandResponse::Signaled), "signaled", &killed)?;
        l.wait(&first).await?;
        let restarted = outcomes::started(l.start_process(argv, spec()).await?)?;
        ensure(restarted != first, "the name to be freed after exit", &restarted)?;
        let _ = l.kill(&restarted).await;
        Ok(())
    })
}

fn only_running_process(listed: &ListProcessesResponse, name: &str, id: &ExecutionId) -> bool {
    let ListProcessesResponse::Processes { processes } = listed else {
        return false;
    };
    let named = processes
        .iter()
        .filter(|p| p.name.as_ref().map(|n| n.as_str()) == Some(name));
    named.count() == 1
        && processes
            .iter()
            .any(|p| p.execution_id == *id && p.state == ProcessState::Running)
}

pub(crate) fn process_readiness_reports_ready_not_ready_and_failed<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let argv = &l.target.process_command;

        let id = outcomes::started(l.start_process(argv, output_contains(&l.target.process_needle)).await?)?;
        let ready = l.wait_ready(&id, 10).await?;
        ensure(matches!(ready, WaitReadyResponse::Ready), "ready", &ready)?;
        let _ = l.kill(&id).await;

        let silent = output_contains("text-that-is-never-printed");
        let started = l.start_process(argv, silent).await?;
        let id = outcomes::started(started)?;
        let waiting = l.wait_ready(&id, 1).await?;
        ensure(matches!(waiting, WaitReadyResponse::NotReady), "not_ready", &waiting)?;
        let _ = l.kill(&id).await;

        let failing = vec!["false".to_string()];
        let id = outcomes::started(l.start_process(&failing, output_contains("never")).await?)?;
        let failed = l.wait_ready(&id, 10).await?;
        let ok = matches!(failed, WaitReadyResponse::Failed { exit_code } if exit_code != 0);
        ensure(ok, "failed", &failed)?;

        let Some((listener, port)) = &l.target.process_listener else {
            return Ok(());
        };
        probe_port(l, listener, *port).await
    })
}

fn output_contains(needle: &str) -> ProcessSpec<'_> {
    ProcessSpec::ready_when(Readiness::OutputContains { needle })
}

async fn probe_port(l: &Held<'_>, listener: &[String], port: u16) -> Result<(), CheckFailureError> {
    let probe = ProcessSpec::ready_when(Readiness::TcpPort { port });
    let started = l.start_process(listener, probe).await?;
    let id = outcomes::started(started)?;
    let ready = l.wait_ready(&id, 10).await?;
    ensure(matches!(ready, WaitReadyResponse::Ready), "a ready port", &ready)?;
    let _ = l.kill(&id).await;
    Ok(())
}

#[cfg(test)]
mod tests;
